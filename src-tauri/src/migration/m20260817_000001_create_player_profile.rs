use sea_orm_migration::prelude::*;

/// 创建玩家档案表（player_profile）。
///
/// 背景：解耦玩家与 AI 概念。玩家身份不再存放在各 AI 角色的 settings.yml 中，
/// 而是独立存放在此表。全局唯一一份，所有 AI 角色统一读取。
///
/// 设计说明：
/// - `id` 固定为 1（单玩家模型），用 `INSERT OR REPLACE` 更新
/// - `user_prompt` 预留玩家系统提示词字段（后续用于玩家侧人格/语气配置）
/// - 纯增量表，不影响现有表结构
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PlayerProfile::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(PlayerProfile::Id).integer().primary_key())
                    .col(ColumnDef::new(PlayerProfile::UserName).string().not_null().default("玩家"))
                    .col(ColumnDef::new(PlayerProfile::UserSubtitle).string().null())
                    .col(ColumnDef::new(PlayerProfile::UserPrompt).string().null())
                    .col(ColumnDef::new(PlayerProfile::UpdatedAt).timestamp().null())
                    .to_owned(),
            )
            .await?;

        // 插入默认玩家档案行（id=1），保证查询始终有值
        manager
            .get_connection()
            .execute(
                sea_orm::Statement::from_string(
                    manager.get_database_backend(),
                    "INSERT OR IGNORE INTO player_profile (id, user_name, user_subtitle, user_prompt) VALUES (1, '玩家', '', '')".to_string(),
                ),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PlayerProfile::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PlayerProfile {
    Table,
    Id,
    UserName,
    UserSubtitle,
    UserPrompt,
    UpdatedAt,
}
