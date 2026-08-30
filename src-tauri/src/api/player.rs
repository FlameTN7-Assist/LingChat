use tauri::{AppHandle, Manager};

use crate::AppState;
use crate::db::managers::player_profile_repo::PlayerProfileRepo;

/// 读取全局玩家档案。
#[tauri::command]
pub async fn get_player_profile(app: AppHandle) -> Result<serde_json::Value, String> {
    let state = app.state::<AppState>();
    let profile = PlayerProfileRepo::get_profile(&state.db)
        .await
        .map_err(|e| format!("读取玩家档案失败: {e}"))?;
    Ok(serde_json::json!({
        "user_name": profile.user_name,
        "user_subtitle": profile.user_subtitle.unwrap_or_default(),
        "user_prompt": profile.user_prompt.unwrap_or_default(),
    }))
}

/// 保存全局玩家档案。
#[tauri::command]
pub async fn set_player_profile(
    app: AppHandle,
    user_name: String,
    user_subtitle: Option<String>,
    user_prompt: Option<String>,
) -> Result<serde_json::Value, String> {
    let state = app.state::<AppState>();
    PlayerProfileRepo::save_profile(&state.db, user_name, user_subtitle, user_prompt)
        .await
        .map_err(|e| format!("保存玩家档案失败: {e}"))?;
    Ok(serde_json::json!({"success": true}))
}
