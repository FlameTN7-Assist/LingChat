use anyhow::Result;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::db::entities::player_profile;

/// 玩家档案仓库：全局唯一玩家配置的 CRUD 操作。
///
/// 解耦玩家与 AI 概念：玩家身份（名字/副标题/prompt）独立存储在
/// `player_profile` 表，不再从各 AI 角色的 settings.yml 中读取。
pub struct PlayerProfileRepo;

impl PlayerProfileRepo {
    /// 读取玩家档案。
    ///
    /// 表中默认有 id=1 的行（创建表时插入）。若行不存在（极端情况），
    /// 返回默认值 `("玩家", "", "")`，保证调用方始终拿到有效数据。
    pub async fn get_profile(db: &DatabaseConnection) -> Result<player_profile::Model> {
        let profile = player_profile::Entity::find()
            .filter(player_profile::Column::Id.eq(1))
            .one(db)
            .await?
            .unwrap_or_else(|| player_profile::Model {
                id: 1,
                user_name: "玩家".to_string(),
                user_subtitle: None,
                user_prompt: None,
                updated_at: None,
            });
        Ok(profile)
    }

    /// 更新玩家档案（INSERT OR REPLACE 语义，id 固定为 1）。
    pub async fn save_profile(
        db: &DatabaseConnection,
        user_name: String,
        user_subtitle: Option<String>,
        user_prompt: Option<String>,
    ) -> Result<()> {
        let now = chrono::Local::now().naive_local();

        let model = player_profile::ActiveModel {
            id: Set(1),
            user_name: Set(user_name),
            user_subtitle: Set(user_subtitle),
            user_prompt: Set(user_prompt),
            updated_at: Set(Some(now)),
        };

        // 先删后插，保证 id=1 只有一行（SQLite 的 UPSERT 需要额外语法）
        let _ = player_profile::Entity::delete_by_id(1).exec(db).await;
        model.insert(db).await?;

        Ok(())
    }

    /// 更新玩家名（为剧本脚本临时覆盖玩家身份提供的便捷方法）。
    pub async fn update_user_name(db: &DatabaseConnection, user_name: String) -> Result<()> {
        let profile = Self::get_profile(db).await?;
        Self::save_profile(db, user_name, profile.user_subtitle, profile.user_prompt).await
    }
}
