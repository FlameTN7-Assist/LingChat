//! AI 对话事件 —— 设定角色，并通过 MessageGenerator 生成 AI 回复。

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::Value;
use tauri::Manager;

use crate::ai_service::game_system::script_engine::events::{
    generate_with_retry, parse_duration, register_event, ScriptContext, ScriptEvent,
};
use crate::ai_service::game_system::script_engine::utils::script_function;
use crate::ai_service::message_system::generator::{
    GeneratorDeps, GeneratorSource, MessageGenerator,
};
use crate::ai_service::types::{LineAttributeExt, LineBase};
use crate::db::entities::line::LineAttribute;
use crate::utils::prompt::PromptRole;
use crate::AppState;

pub struct AIDialogueEvent {
    character: String,
    prompt: Option<String>,
    duration: Option<f64>,
}

impl AIDialogueEvent {
    fn from_event_data(data: &Value) -> Self {
        Self {
            character: data
                .get("character")
                .and_then(|v| v.as_str())
                .unwrap_or("MAIN")
                .to_string(),
            prompt: data
                .get("prompt")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            duration: parse_duration(data),
        }
    }
}

#[async_trait]
impl ScriptEvent for AIDialogueEvent {
    async fn execute(&mut self, ctx: &mut ScriptContext<'_>) -> Result<Option<String>> {
        let script_status = ctx
            .game_status
            .lock()
            .await
            .script_status
            .clone()
            .ok_or_else(|| anyhow!("ScriptStatus 未设置"))?;

        let role_id = {
            let mut gs = ctx.game_status.lock().await;
            let role = script_function::get_role(&mut *gs, ctx.db, &script_status, &self.character)
                .await?;
            role.role_id.ok_or_else(|| anyhow!("角色 ID 未设置"))?
        };

        // 设为当前角色
        ctx.game_status.lock().await.current_role_id = Some(role_id);

        tracing::info!("[AIDialogueEvent] 开始执行");

        // 若提供了 prompt，作为临时系统旁白台词注入
        // TODO: 这里的 prompt 是暂时的，应该标记为临时 prompt，并且在代码逻辑中在AI回复后清除这部分提示词。
        if let Some(ref prompt) = self.prompt {
            let sys_line = LineBase {
                content: PromptRole::Plot.build_prompt(prompt),
                attribute: LineAttributeExt(LineAttribute::User),
                display_name: Some("旁白".to_string()),
                ..Default::default()
            };
            ctx.game_status
                .lock()
                .await
                .add_line(ctx.db, sys_line)
                .await?;
        }

        // 委托 MessageGenerator 生成回复
        let state = ctx.app.state::<AppState>();
        let llm = crate::ai_service::llm::slot_snapshot(&state.chat.llm).await;
        let llm = match llm {
            Some(llm) => llm,
            None => {
                // LLM 未配置：AI 对话事件无法生成。按上游要求直接终止剧本，
                // 不再 fallback 到任何占位/默认文本——那会让剧本以错误逻辑继续跑。
                return Err(anyhow!(
                    "尚未配置大模型，无法执行「AI 对话」事件，剧本终止。请先在设置里配置并选择模型。"
                ));
            }
        };

        let deps = GeneratorDeps {
            source: GeneratorSource::ScriptAiDialogue,
            app: ctx.app.clone(),
            db: ctx.db.clone(),
            game_status: ctx.game_status.clone(),
            processor: state.chat.processor.clone(),
            translator: state.chat.translator.clone(),
            llm,
            tool_registry: state.tool_registry.clone(),
            concurrency: 1,
            god_agent: None,
            suppress_thinking: false,
            // 捕获当前试玩代号：中止后游离任务再写会被 add_assistant_line 的守卫丢弃
            generation: ctx.game_status.lock().await.preview_generation,
            is_preview: ctx.is_preview,
        };

        let generator = MessageGenerator::new(deps);

        // LLM 调用失败**绝不踢出玩家**：自动重试 3 次后广播「重试」提示、等玩家点
        // 「继续」再试（详见 generate_with_retry 的注释：不跳过对话、不退出剧本，
        // 重试回溯点正确——进度停在当前事件、line_list 无残留）。
        generate_with_retry(ctx, &generator).await?;

        // 记录该 AI 对话的回复已完整生成（key 带事件索引）：
        // 读档续跑时据此精确判断「是否跳过该事件」（不重新调 LLM）——比用
        // line_list 末尾 assistant 粗判断更可靠（连续 ai_dialogue 时末尾可能是
        // 上一个事件的回复，粗判断会误判）。中断/失败未生成完则不会记录。
        {
            let mut gs = ctx.game_status.lock().await;
            if let Some(ref mut ss) = gs.script_status {
                ss.vars.insert(
                    format!("__ai_reply_{}", ss.current_event_process),
                    serde_json::json!(true),
                );
            }
        }

        tracing::info!("[AIDialogueEvent] 执行完毕");

        Ok(None)
    }

    fn event_type() -> &'static str {
        "ai_dialogue"
    }

    fn duration(&self) -> Option<f64> {
        self.duration
    }
}

pub fn register() {
    register_event(AIDialogueEvent::event_type(), |data| {
        Box::new(AIDialogueEvent::from_event_data(&data))
    });
}
