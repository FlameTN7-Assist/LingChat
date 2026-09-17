//! 玩家身份（`role_type=User` 的统一实体）命令层。
//!
//! 统一实体后玩家身份与 AI 角色同为 `role` 行，这里只暴露"玩家身份"这一侧：
//! 列表/新建/改名改人设/删除，以及运行时附身切换。附身本身是会话态，
//! 具体逻辑委托给 `possession` 模块，命令层只负责参数校验、缓存刷新与事件广播。

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;
use crate::ai_service::game_system::possession;
use crate::ai_service::types::{PLAYER_ROLE_ID, RoleProfile};
use crate::config::AppConfig;
use crate::db::managers::role_repo::RoleRepo;
use crate::utils::prompt::PromptOptions;

/// 一条玩家身份（含人设与是否为当前被附身实体）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct IdentityInfo {
    pub role_id: i32,
    pub name: String,
    pub profile: RoleProfile,
    pub possessed: bool,
}

/// 当前被附身实体的简要信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PossessedInfo {
    pub role_id: i32,
    pub name: String,
    pub subtitle: String,
}

/// 提示词选项：与正式游玩保持同一来源，保证重建 SYSTEM 行不改变人设格式。
fn prompt_options(app: &AppHandle) -> PromptOptions {
    let config = AppConfig::load(app).unwrap_or_default();
    PromptOptions {
        output_sec_lang: config.llm_output_sec_lang,
        no_emotion_limit: config.no_emotion_limit_prompt,
    }
}

/// 读取当前被附身实体 id（不做任何加锁以外的副作用）。
async fn current_possessed(app: &AppHandle) -> i32 {
    let state = app.state::<AppState>();
    let service = state.ai_service.lock().await;
    let gs = service.game_status.lock().await;
    gs.possessed_role_id
}

#[tauri::command]
pub async fn list_identities(app: AppHandle) -> Result<Vec<IdentityInfo>, String> {
    let state = app.state::<AppState>();
    let identities = RoleRepo::list_player_identities(&state.db)
        .await
        .map_err(|e| format!("获取玩家身份列表失败: {}", e))?;
    let possessed = current_possessed(&app).await;
    Ok(identities
        .into_iter()
        .map(|(role, profile)| IdentityInfo {
            role_id: role.id,
            name: role.name,
            profile,
            possessed: role.id == possessed,
        })
        .collect())
}

#[tauri::command]
pub async fn create_identity(
    app: AppHandle,
    name: String,
    profile: RoleProfile,
) -> Result<i32, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("身份名称不能为空".to_string());
    }
    let state = app.state::<AppState>();
    RoleRepo::create_player_identity(&state.db, name, &profile)
        .await
        .map_err(|e| format!("创建玩家身份失败: {}", e))
}

#[tauri::command]
pub async fn update_identity(
    app: AppHandle,
    role_id: i32,
    name: String,
    profile: RoleProfile,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("身份名称不能为空".to_string());
    }
    let state = app.state::<AppState>();
    RoleRepo::update_player_identity(&state.db, role_id, name, &profile)
        .await
        .map_err(|e| format!("更新玩家身份失败: {}", e))?;

    // 改名/改人设会改变玩家缓存与嵌在 AI 人设里的名字，只有被附身实体需要立刻同步；
    // 其它身份下一次被附身时会走同一条刷新路径。
    if current_possessed(&app).await == role_id {
        let options = prompt_options(&app);
        let service = state.ai_service.lock().await;
        let mut gs = service.game_status.lock().await;
        gs.refresh_possessed_cache(&state.db)
            .await
            .map_err(|e| format!("刷新玩家缓存失败: {}", e))?;
        possession::rebuild_system_lines(&mut gs, &state.db, options)
            .await
            .map_err(|e| format!("重建角色人设失败: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_identity(app: AppHandle, role_id: i32) -> Result<bool, String> {
    let state = app.state::<AppState>();
    let was_possessed = current_possessed(&app).await == role_id;
    let deleted = RoleRepo::delete_player_identity(&state.db, role_id)
        .await
        .map_err(|e| format!("删除玩家身份失败: {}", e))?;

    // 被附身实体被删后 possessed 会指向不存在的行，必须回落到默认身份，
    // 否则后续发言归属与缓存刷新都会落空。
    if deleted && was_possessed {
        let options = prompt_options(&app);
        let service = state.ai_service.lock().await;
        let mut gs = service.game_status.lock().await;
        gs.possessed_role_id = PLAYER_ROLE_ID;
        gs.refresh_possessed_cache(&state.db)
            .await
            .map_err(|e| format!("刷新玩家缓存失败: {}", e))?;
        possession::rebuild_system_lines(&mut gs, &state.db, options)
            .await
            .map_err(|e| format!("重建角色人设失败: {}", e))?;
    }
    Ok(deleted)
}

#[tauri::command]
pub async fn possess_entity(app: AppHandle, role_id: i32) -> Result<String, String> {
    let state = app.state::<AppState>();
    let options = prompt_options(&app);
    let name = {
        let service = state.ai_service.lock().await;
        let mut gs = service.game_status.lock().await;
        possession::possess_entity(&mut gs, &state.db, role_id, options)
            .await
            .map_err(|e| format!("附身失败: {}", e))?
    };

    if let Err(e) = app.emit(
        "identity:possessed",
        serde_json::json!({ "role_id": role_id, "name": name.clone() }),
    ) {
        tracing::warn!("emit identity:possessed 失败: {e}");
    }
    Ok(name)
}

#[tauri::command]
pub async fn get_possessed_entity(app: AppHandle) -> Result<PossessedInfo, String> {
    let state = app.state::<AppState>();
    let service = state.ai_service.lock().await;
    let gs = service.game_status.lock().await;
    Ok(PossessedInfo {
        role_id: gs.possessed_role_id,
        name: gs.player.user_name.clone(),
        subtitle: gs.player.user_subtitle.clone(),
    })
}
