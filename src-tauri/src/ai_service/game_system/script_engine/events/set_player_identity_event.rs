//! Set player identity event — 在剧本运行中切换玩家身份（叙事/对话视角）。
//!
//! 解耦玩家与 AI 设定后的剧本级身份切换机制。剧本作者可在章节中临时把玩家
//! 变成「另一个人」（如视角切换），支持：
//! - `user_name` / `user_subtitle` / `user_prompt`：新的玩家身份字段（均可选）
//! - `scope`：`"chapter"`（默认，章节结束后还原）/ `"script"`（剧本结束后还原）/
//!   `"permanent"`（永久生效，不还原）
//!
//! 实现原理：把当前 `GameStatus.player` 存入 `player_identity_override`，
//! 剧本的 chapter_end / on_script_end 统一检查该字段并还原。

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::ai_service::game_system::script_engine::events::{
    register_event, ScriptContext, ScriptEvent,
};

pub struct SetPlayerIdentityEvent {
    user_name: Option<String>,
    user_subtitle: Option<String>,
    user_prompt: Option<String>,
    scope: String,
}

impl SetPlayerIdentityEvent {
    fn from_event_data(data: &Value) -> Self {
        Self {
            user_name: data.get("user_name").and_then(|v| v.as_str()).map(|s| s.to_string()),
            user_subtitle: data
                .get("user_subtitle")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            user_prompt: data
                .get("user_prompt")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            scope: data
                .get("scope")
                .and_then(|v| v.as_str())
                .unwrap_or("chapter")
                .to_string(),
        }
    }
}

#[async_trait]
impl ScriptEvent for SetPlayerIdentityEvent {
    async fn execute(&mut self, ctx: &mut ScriptContext<'_>) -> Result<Option<String>> {
        let mut gs = ctx.game_status.lock().await;

        // 保存原始玩家身份（仅当还没存过时）。之后 chapter_end / on_script_end 会捕获
        // `player_identity_override` 并还原，支持跨多轮保持（用户要求的「保持多轮不变」）。
        if gs.player_identity_override.is_none() && self.scope != "permanent" {
            gs.player_identity_override = Some(gs.player.clone());
        }

        // 应用新的玩家身份（只覆盖提供的字段）
        if let Some(name) = &self.user_name {
            if !name.is_empty() {
                gs.player.user_name = name.clone();
            }
        }
        if let Some(subtitle) = &self.user_subtitle {
            gs.player.user_subtitle = subtitle.clone();
        }
        if let Some(prompt) = &self.user_prompt {
            gs.player.user_prompt = prompt.clone();
        }

        tracing::info!(
            "[SetPlayerIdentityEvent] 玩家身份切换为 '{}' (scope={})",
            gs.player.user_name,
            self.scope,
        );

        Ok(None)
    }

    fn event_type() -> &'static str {
        "set_player_identity"
    }
}

pub fn register() {
    register_event(SetPlayerIdentityEvent::event_type(), |data| {
        Box::new(SetPlayerIdentityEvent::from_event_data(&data))
    });
}
