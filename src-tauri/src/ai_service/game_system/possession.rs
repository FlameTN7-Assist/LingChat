//! 运行时附身：把「玩家」从字面值 0 哨兵升级为统一实体模型下的会话态。
//!
//! 附身只改动 `GameStatus.possessed_role_id` 及由其派生的缓存与提示词，**不写回**
//! `role.role_type`——实体是不是 AI，与"此刻谁在扮演它"是两件互不影响的事。
//! 附身期间玩家以被附身实体的身份发言（玩家台词 `sender_role_id` = 该实体），
//! 该实体的 AI 生成休眠；切走后它恢复 AI 控制，期间台词按 sender 天然留在其记忆中。

use anyhow::{anyhow, Result};
use sea_orm::DatabaseConnection;

use crate::ai_service::game_system::game_status::GameStatus;
use crate::ai_service::game_system::role_manager::user_identity_settings;
use crate::ai_service::types::PLAYER_ROLE_ID;
use crate::db::entities::line::LineAttribute;
use crate::db::entities::role::RoleType;
use crate::db::managers::role_repo::RoleRepo;
use crate::utils::prompt::{PromptOptions, sys_prompt_builder_by_settings};

/// 把玩家附身到指定实体上，返回该实体的显示名。
///
/// 附身后：
/// 1. 实体必须"在场"（否则感知不到台词，God Agent 也看不见它）；
/// 2. 若当前对话对象恰好是该实体（自己跟自己说话的死锁），自动切到默认身份或
///    在场的另一个非附身实体；
/// 3. 重建全部 SYSTEM 人设行（玩家名嵌在每条 AI 人设里）。
pub async fn possess_entity(
    gs: &mut GameStatus,
    db: &DatabaseConnection,
    role_id: i32,
    prompt_options: PromptOptions,
) -> Result<String> {
    RoleRepo::get_role_by_id(db, role_id)
        .await?
        .ok_or_else(|| anyhow!("实体不存在: role_id={}", role_id))?;

    gs.possessed_role_id = role_id;
    gs.refresh_possessed_cache(db).await?;

    // 确保被附身实体已加载到运行时：God Agent 简介、记忆同步都从 role_manager 取数据；
    // 对 User 实体这里会走合成 settings 路径（没有 settings.yml 也能加载）。
    let _ = gs.get_role(db, role_id).await?;

    // 只补 present 不进 onstage：玩家身份实体没有立绘资源，上台会让前端渲染空位。
    if !gs.present_role_ids.contains(&role_id) {
        gs.present_role_ids.insert(role_id);
    }

    // 当前对话对象 = 刚被附身的实体：此时继续生成就是"自己对自己说话"。
    // 默认身份 id=0 可作为 AI 扮演的玩家分身接话；若连它都被附身，
    // 退而求其次选在场另一个非附身实体。
    if gs.current_role_id == Some(role_id) {
        let fallback = if role_id != PLAYER_ROLE_ID {
            Some(PLAYER_ROLE_ID)
        } else {
            gs.present_role_ids.iter().copied().find(|id| *id != role_id)
        };
        gs.current_role_id = fallback;
    }

    rebuild_system_lines(gs, db, prompt_options).await?;

    Ok(gs.player.user_name.clone())
}

/// 按最新附身信息重建全部 SYSTEM 人设行。
///
/// 为什么必须重建：统一实体后"玩家名"由 `sys_prompt_builder` 的 framing 前缀嵌进
/// 每条 AI 人设，附身切换/身份改名后不同步，模型会继续以为在跟旧名字的人说话。
/// 做法是按行 sender 重新调用构建器，**保留行 id 只替换 content**——严禁字符串替换，
/// 否则既可能漏改格式提示里的名字，也可能误伤正文中恰好同名的词。
///
/// AI 角色优先取内存中已加载的最新 settings，未加载才回盘；User 实体用合成 settings。
pub async fn rebuild_system_lines(
    gs: &mut GameStatus,
    db: &DatabaseConnection,
    prompt_options: PromptOptions,
) -> Result<()> {
    let data_dir = crate::api::data_dir();
    let player_name = gs.player.user_name.clone();

    let system_indices: Vec<usize> = gs
        .line_list
        .iter()
        .enumerate()
        .filter(|(_, line)| matches!(line.attribute(), LineAttribute::System))
        .map(|(index, _)| index)
        .collect();

    for index in system_indices {
        let Some(role_id) = gs.line_list[index].base.sender_role_id else {
            continue;
        };
        let Some(role) = RoleRepo::get_role_by_id(db, role_id).await? else {
            continue;
        };
        // 先取出内存 settings 并立即释放对 role_manager 的借用，避免跨越磁盘查询的 await
        let loaded_settings = gs
            .role_manager
            .get_loaded(role_id)
            .map(|loaded| loaded.settings.clone());
        let settings = if role.role_type == RoleType::User {
            let profile = RoleRepo::get_role_profile(db, role_id).await?;
            user_identity_settings(&role, &profile)
        } else if let Some(settings) = loaded_settings {
            settings
        } else {
            match RoleRepo::get_role_settings_by_id(db, &data_dir, role_id).await? {
                Some(settings) => settings,
                None => continue,
            }
        };
        gs.line_list[index].base.content =
            sys_prompt_builder_by_settings(&settings, prompt_options, &player_name);
    }

    gs.refresh_memories(db).await?;
    Ok(())
}
