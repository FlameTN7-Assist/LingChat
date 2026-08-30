use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 玩家档案（全局唯一）。
///
/// 解耦玩家与 AI 概念：玩家身份不再存放在各 AI 角色的 settings.yml 中，
/// 而是独立存放在此表。所有 AI 角色统一从此表读取玩家名/副标题。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "player_profile")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub user_name: String,
    pub user_subtitle: Option<String>,
    pub user_prompt: Option<String>,
    pub updated_at: Option<DateTime>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
