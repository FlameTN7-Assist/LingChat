//! Serializable payload types for Tauri script events.
//!
//! Each struct matches the payload shape expected by the frontend event processors
//! in `src/core/events/processors/`. The frontend's `asEvent()` helper merges in
//! `type` and `duration` fields, so those are omitted here.

use serde::Serialize;

// ============================================================
// Tauri event name constants
// ============================================================

pub mod event_names {
    pub const SCRIPT_NARRATION: &str = "script:narration";
    pub const SCRIPT_PLAYER: &str = "script:player";
    pub const SCRIPT_CHAPTER_CHANGE: &str = "script:chapter-change";
    pub const SCRIPT_BACKGROUND: &str = "script:background";
    pub const SCRIPT_BACKGROUND_EFFECT: &str = "script:background-effect";
    pub const SCRIPT_MUSIC: &str = "script:music";
    pub const SCRIPT_SOUND: &str = "script:sound";
    pub const SCRIPT_AMBIENT: &str = "script:ambient";
    pub const SCRIPT_PRESENT_PIC: &str = "script:present-pic";
    pub const SCRIPT_MODIFY_CHARACTER: &str = "script:modify-character";
    pub const SCRIPT_INPUT: &str = "script:input";
    pub const SCRIPT_CHOICE: &str = "script:choice";
    pub const SCRIPT_END: &str = "script:end";
    pub const SCRIPT_FREE_DIALOGUE: &str = "script:free-dialogue";
    /// 事件进度广播：每实际执行一个事件，把（章节 key + 事件序号）推给前端，
    /// 前端据此记录「玩家阅读位置」，读档按玩家位置续跑（而非引擎执行位置）。
    pub const SCRIPT_PROGRESS: &str = "script:progress";
    /// LLM 生成失败、等待玩家「继续」重试（AI 对话事件在自动重试耗尽后广播）。
    pub const SCRIPT_LLM_RETRY: &str = "script:llm-retry";
}

// ============================================================
// Payload types (fields match frontend `src/types/script.ts`)
// ============================================================

/// 剧本事件进度（script:progress）——引擎实际执行每个事件前广播一次。
/// 不阻塞引擎（预跑不受影响）；前端在队列里与其后的内容事件相邻、保序消费。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptProgressPayload {
    /// 当前章节 key（YAML 文件名，如 "main" / "Intro/intro"）
    pub chapter: String,
    /// 事件在章节事件数组中的序号
    pub seq: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NarrationPayload {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// 事件间隔（秒），来自 YAML 的 `duration`；None = 前端按类型默认节奏
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerPayload {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterChangePayload {
    pub chapter_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundPayload {
    pub image_path: String,
    #[serde(default)]
    pub transition: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundEffectPayload {
    pub effect: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicPayload {
    pub music_path: String,
    /// 播放速度倍率（1.0 原速）；None 表示未设置，前端按 1.0 处理
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback_speed: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundPayload {
    pub sound_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AmbientPayload {
    pub ambient_path: String,
    #[serde(default = "default_ambient_volume")]
    pub volume: f64,
    #[serde(default = "default_true", rename = "loop")]
    pub is_loop: bool,
    #[serde(default)]
    pub stop: bool,
    /// 是否启用淡入淡出，默认 true
    #[serde(default = "default_true")]
    pub fade: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentPicPayload {
    pub image_path: String,
    #[serde(default = "default_scale")]
    pub scale: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

#[allow(dead_code)]
fn default_scale() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModifyCharacterPayload {
    pub character_id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clothes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputPayload {
    pub hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

/// 单个选项：文案 + 是否因条件不满足而不可选 + 不可选时的提示（lock_hint）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChoiceItem {
    pub text: String,
    /// 条件不满足时为 true，前端应灰显并禁止点击
    #[serde(default)]
    pub disabled: bool,
    /// 作者写的锁定提示文案（lock_hint）；没有时前端给默认文案
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChoicePayload {
    pub choices: Vec<ChoiceItem>,
    #[serde(default)]
    #[serde(rename = "allowFree")]
    pub allow_free: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreeDialoguePayload {
    #[serde(rename = "switch")]
    pub switch: bool,
    pub max_rounds: i32,
    pub end_line: String,
    /// 读档续轮时已进行的轮次（saved_rounds）。前端据此同步 currentRound 显示，
    /// 避免续跑后前端提示停留在 0/N 而后端已到第 N 轮。正常新跑时为 0。
    #[serde(default)]
    pub current_round: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptEndPayload {
    /// `false` when the script was torn down because of an error rather than
    /// reaching its end. The frontend must not credit the player with an
    /// adventure completion in that case.
    pub completed: bool,
}

/// LLM 生成失败、等待玩家点击「继续」重试（script:llm-retry）。
/// 前端收到后显示提示文本并进入「等待继续」状态；玩家点击继续经
/// `script_event_continue` 回执到 continue_tx，引擎据此重新调用 LLM。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmRetryPayload {
    /// 展示给玩家的提示文案（如「AI 响应失败，点击继续重试」）
    pub message: String,
}
