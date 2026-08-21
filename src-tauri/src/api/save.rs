use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::ai_service::game_system::game_status::GameStatusSnapshot;
use crate::api::game::build_web_init_data;
use crate::api::game::WebInitData;
use crate::config::AppConfig;
use crate::db::entities::line::LineAttribute;
use crate::db::managers::role_repo::RoleRepo;
use crate::db::managers::save_repo::SaveRepo;
use crate::utils::prompt::PromptOptions;
use crate::AppState;

// ========== 响应类型 ==========

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SaveListItem {
    pub id: i32,
    pub title: String,
    pub create_date: String,
    pub update_date: String,
    pub last_message: Option<String>,
    pub screenshot: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SaveListResponse {
    pub saves: Vec<SaveListItem>,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CreateSaveResponse {
    pub save_id: i32,
    pub message: String,
}

// ========== 辅助函数 ==========

fn format_datetime(dt: &chrono::NaiveDateTime) -> String {
    dt.and_utc().to_rfc3339()
}

async fn save_screenshot_file(save_id: i32, source_path: &str) -> Result<(), String> {
    let screenshots_dir = super::data_dir().join("screenshots");
    std::fs::create_dir_all(&screenshots_dir).map_err(|e| e.to_string())?;
    let dest_path = screenshots_dir.join(format!("{}.png", save_id));
    std::fs::copy(source_path, &dest_path)
        .map_err(|e| format!("复制截图文件失败: {} → {:?}: {}", source_path, dest_path, e))?;
    Ok(())
}

// ========== Tauri 命令 ==========

#[tauri::command]
pub async fn list_saves(
    app: AppHandle,
    page: Option<u64>,
    page_size: Option<u64>,
) -> Result<SaveListResponse, String> {
    let state = app.state::<AppState>();
    let db = &state.db;
    let page = page.unwrap_or(1);
    let page_size = page_size.unwrap_or(50);

    let total = SaveRepo::count_saves(db)
        .await
        .map_err(|e| format!("查询存档总数失败: {}", e))?;

    let saves = SaveRepo::list_saves(db, page, page_size)
        .await
        .map_err(|e| format!("查询存档列表失败: {}", e))?;

    // 1. 获取所有 last_message_id 并批量查询内容
    let last_msg_ids: Vec<i32> = saves.iter().filter_map(|s| s.last_message_id).collect();
    let mut lines_map = std::collections::HashMap::new();
    if !last_msg_ids.is_empty() {
        use crate::db::entities::line;
        use sea_orm::entity::prelude::*;
        if let Ok(lines) = line::Entity::find()
            .filter(line::Column::Id.is_in(last_msg_ids))
            .all(db)
            .await
        {
            for l in lines {
                lines_map.insert(l.id, l.content);
            }
        }
    }

    let data_dir = super::data_dir();
    let screenshots_dir = data_dir.join("screenshots");

    let items: Vec<SaveListItem> = saves
        .into_iter()
        .map(|s| {
            let last_message = s.last_message_id.and_then(|id| lines_map.get(&id).cloned());
            let screenshot_path = screenshots_dir.join(format!("{}.png", s.id));
            let screenshot = if screenshot_path.exists() {
                Some(screenshot_path.to_string_lossy().to_string())
            } else {
                None
            };

            SaveListItem {
                id: s.id,
                title: s.title,
                create_date: format_datetime(&s.create_date),
                update_date: format_datetime(&s.update_date),
                last_message,
                screenshot,
            }
        })
        .collect();

    Ok(SaveListResponse {
        saves: items,
        total,
    })
}

#[tauri::command]
pub async fn create_save(
    app: AppHandle,
    title: String,
    screenshot_path: Option<String>,
) -> Result<CreateSaveResponse, String> {
    let state = app.state::<AppState>();
    let db = &state.db;

    let mut service = state.ai_service.lock().await;
    let lines = service.game_status.lock().await.line_list.clone();

    // 1. 创建 save 行
    let save_model = SaveRepo::create_save(db, &title)
        .await
        .map_err(|e| format!("创建存档失败: {}", e))?;
    let save_id = save_model.id;

    // 复制截图到 screenshots 目录
    if let Some(ref path) = screenshot_path {
        let _ = save_screenshot_file(save_id, path).await;
    }

    // 2. 同步台词
    if !lines.is_empty() {
        SaveRepo::sync_lines(db, save_id, &lines)
            .await
            .map_err(|e| format!("同步台词失败: {}", e))?;
    }

    // 3. 设置主角
    if let Some(main_id) = service.game_status.lock().await.main_role_id {
        SaveRepo::update_save_main_role(db, save_id, Some(main_id))
            .await
            .map_err(|e| format!("设置主角失败: {}", e))?;
    }

    // 4. 写入 GameStatus 快照
    let snapshot = service.game_status.lock().await.to_snapshot();
    let snapshot_json =
        serde_json::to_string(&snapshot).map_err(|e| format!("序列化状态失败: {}", e))?;
    SaveRepo::update_save_status(db, save_id, &snapshot_json)
        .await
        .map_err(|e| format!("保存状态失败: {}", e))?;

    // 5. 标记当前活跃存档
    service.game_status.lock().await.active_save_id = Some(save_id);

    // 6. 持久化 MemoryBank
    service
        .persist_memory_banks(save_id)
        .await
        .map_err(|e| format!("保存记忆库失败: {}", e))?;

    // 7. 持久化剧本状态（若有）
    {
        let gs = service.game_status.lock().await;
        if let Some(ref script_status) = gs.script_status {
            let vars_json = serde_json::to_string(&script_status.vars).unwrap_or_default();
            // 玩家阅读位置（前端上报）：与引擎位置（event_sequence）并存，读档优先据此恢复。
            // 剧本未开始时为空（init_script 已清），写 None 表示无上报记录，读档回退引擎位置。
            let player_read_chapter = if gs.player_read_chapter.is_empty() {
                None
            } else {
                Some(gs.player_read_chapter.clone())
            };
            // 以 player_read_chapter 非空为「是否上报过」标志：首个内容事件的事件
            // 序号可能是 0（如剧本第一行旁白），不能因 seq>0 而漏存——否则读档
            // 时玩家位置为空，只能回退引擎位置。
            let player_read_sequence = if gs.player_read_chapter.is_empty() {
                None
            } else {
                Some(gs.player_read_seq.max(0))
            };
            let _ = SaveRepo::upsert_running_script(
                db,
                save_id,
                &script_status.folder_key,
                &vars_json,
                &script_status.current_chapter_key,
                script_status.current_event_process,
                player_read_chapter,
                player_read_sequence,
            )
            .await
            .map_err(|e| eprintln!("[SAVE_WARN] create_save: 保存剧本状态失败: {}", e));
        }
    }

    Ok(CreateSaveResponse {
        save_id,
        message: "存档创建成功".into(),
    })
}

#[tauri::command]
pub async fn load_save(app: AppHandle, save_id: i32) -> Result<WebInitData, String> {
    let state = app.state::<AppState>();
    let db = &state.db;

    let mut service = state.ai_service.lock().await;
    let load_started = std::time::Instant::now();

    // 1. 获取存档
    let save_model = SaveRepo::get_save_by_id(db, save_id)
        .await
        .map_err(|e| format!("查询存档失败: {}", e))?
        .ok_or_else(|| format!("存档 {} 不存在", save_id))?;

    // 2. 获取台词列表
    let line_list = SaveRepo::get_gameline_list(db, save_id)
        .await
        .map_err(|e| format!("读取台词失败: {}", e))?;
    tracing::info!(
        "[LoadSave] 读取存档与台词: {} 行, 耗时 {:.0}ms",
        line_list.len(),
        load_started.elapsed().as_millis()
    );

    // 3. 获取主角 role_id
    let main_role_id = save_model
        .main_role_id
        .ok_or_else(|| "存档中未记录主角信息".to_string())?;

    // 4. 加载角色设定
    let data_dir = crate::api::data_dir();
    let settings = RoleRepo::get_role_settings_by_id(db, &data_dir, main_role_id)
        .await
        .map_err(|e| format!("查询角色配置失败: {}", e))?
        .unwrap_or_else(|| {
            let mut s = crate::ai_service::types::CharacterSettings::default();
            s.character_id = Some(main_role_id);
            s
        });

    // 5. 构建 PromptOptions
    let app_config = AppConfig::load(&app).unwrap_or_default();
    let prompt_options = PromptOptions {
        output_sec_lang: app_config.llm_output_sec_lang,
        no_emotion_limit: app_config.no_emotion_limit_prompt,
    };

    // 6. 导入设定并载入台词
    service
        .import_settings(settings.clone(), prompt_options)
        .await;
    service
        .load_lines(line_list, main_role_id, Some(save_id))
        .await
        .map_err(|e| format!("载入台词失败: {}", e))?;

    // 7. 恢复 GameStatus 快照
    let snapshot: GameStatusSnapshot = serde_json::from_str(&save_model.status).unwrap_or_default();
    service.game_status.lock().await.apply_snapshot(&snapshot);

    // 7.5 多人物剧本：预加载在场角色（含剧本 NPC），保证读档后立绘/角色名立即可用。
    //     role_manager.get_loaded 只对已加载角色返回（build_web_init_data 的 onstage_roles
    //     依赖它）；NPC 若暂无记忆不会被第 8 步 restore_memory_banks 自动 get_role，会漏掉。
    {
        let onstage_ids = service.game_status.lock().await.onstage_role_ids.clone();
        for rid in onstage_ids {
            let _ = service.game_status.lock().await.get_role(db, rid).await;
        }
    }

    // 8. 恢复 MemoryBank
    let _ = service
        .restore_memory_banks(save_id)
        .await
        .map_err(|e| eprintln!("[SAVE_WARN] 恢复记忆库失败: {}", e));
    tracing::info!(
        "[LoadSave] 导入设定/载入台词/恢复快照/记忆库完成, 耗时 {:.0}ms",
        load_started.elapsed().as_millis()
    );

    // 8.3 若旧剧本任务仍在运行，abort 其句柄并等待收尾，避免双任务竞争：
    //     旧任务会继续推进事件进度（导致存档事件序号超前于对话内容、续跑跳过剧情），
    //     且新任务设置通道会顶掉旧任务的 sender，旧任务报"通道已关闭"后 teardown 会清掉新恢复的状态。
    if service
        .script_manager
        .is_running
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        tracing::info!("[LoadSave] 检测到旧剧本任务仍在运行，正在中止并等待收尾...");
        let abort_started = std::time::Instant::now();
        let handle = {
            let mut current = service.script_manager.current_run.lock().await;
            current.take()
        };
        if let Some(handle) = handle {
            handle.abort();
            // abort 后任务在其下一个 await 点被取消，此处等待其真正收尾
            let _ = handle.await;
        }
        tracing::info!(
            "[LoadSave] 旧剧本任务中止完成, 耗时 {:.0}ms",
            abort_started.elapsed().as_millis()
        );
        // 清掉通道残留（旧任务可能持有 sender），并复位运行标记：
        // abort 不会走 on_script_end，故 is_running 需要手动复位。
        // continue_tx 一并清掉：旧任务可能正等在旁白「继续」回执上，残留的 sender
        // 会干扰新任务的事件推进。
        let mut ch = state.script_channels.lock().await;
        ch.input_tx = None;
        ch.choice_tx = None;
        ch.continue_tx = None;
        ch.choice_allow_free = false;
        drop(ch);
        service
            .script_manager
            .is_running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        // abort 后旧任务不会执行 on_script_end 清理，显式清掉残留的剧本状态，
        // 下面第 9 步再按存档重建（无剧本的存档则保持 None，不会误报 active_script）。
        service.game_status.lock().await.script_status = None;
    }

    // 8.5 为缺少 system 人设台词的发言/在场 NPC 补建 system 行（缓解"人设丢失"告警）。
    //     仅补内存中的 line_list，下次存档时随 sync_lines 持久化。
    {
        use crate::ai_service::types::{GameLine, LineAttributeExt, LineBase};
        use crate::db::entities::line::LineAttribute;
        let mut gs = service.game_status.lock().await;
        let mut involved: std::collections::HashSet<i32> = std::collections::HashSet::new();
        for l in gs.line_list.iter() {
            if let Some(sid) = l.base.sender_role_id {
                if sid != 0 {
                    involved.insert(sid);
                }
            }
        }
        involved.extend(gs.present_role_ids.iter().copied());
        let has_system: std::collections::HashSet<i32> = gs
            .line_list
            .iter()
            .filter(|l| matches!(l.attribute(), LineAttribute::System))
            .filter_map(|l| l.base.sender_role_id)
            .collect();
        for rid in involved {
            if has_system.contains(&rid) {
                continue;
            }
            let (sys_prompt, display_name) = {
                let role = match gs.role_manager.get_loaded(rid) {
                    Some(r) => r,
                    None => continue,
                };
                if role
                    .settings
                    .system_prompt
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    continue;
                }
                (
                    crate::utils::prompt::sys_prompt_builder_by_settings(
                        &role.settings,
                        prompt_options,
                    ),
                    role.display_name.clone(),
                )
            };
            let sys_line = LineBase {
                content: sys_prompt,
                attribute: LineAttributeExt(LineAttribute::System),
                sender_role_id: Some(rid),
                display_name,
                ..Default::default()
            };
            gs.line_list.insert(0, GameLine::from_base(sys_line, vec![]));
            tracing::info!("[LoadSave] 为角色 {} 补建 system 人设台词（存档缺少）", rid);
        }
    }

    // 9. 恢复剧本状态（若有）——重建 script_status，使剧本可从存档章节续跑。
    //    原实现只查询 running_script 后丢弃，导致读档后剧本永远无法继续。
    if let Some(rs_id) = save_model.running_script_id {
        if let Some(rs) = SaveRepo::get_running_script(db, rs_id)
            .await
            .map_err(|e| format!("查询剧本状态失败: {}", e))?
        {
            let script = service
                .script_manager
                .all_scripts
                .values()
                .find(|s| s.folder_key == rs.script_folder)
                .cloned();
            if let Some(mut script) = script {
                // 优先按「玩家阅读位置」恢复（前端上报，避免从引擎预跑位置续跑导致"跳剧情"）。
                // 仅当玩家位置所在章节在当前剧本目录中真实存在时采用，否则回退引擎位置
                // （剧本内容在存档后被修改、或旧档无上报记录的场景）。
                let chapters_dir = script.script_path.join("Chapters");
                let chapter_exists = |chapter: &str| -> bool {
                    let p = if chapter.ends_with(".yaml") {
                        chapters_dir.join(chapter)
                    } else {
                        chapters_dir.join(format!("{}.yaml", chapter))
                    };
                    p.exists()
                };
                let (restore_chapter, restore_seq, from_player_read) =
                    match rs.player_read_chapter.as_deref() {
                        Some(ch) if !ch.is_empty() && chapter_exists(ch) => (
                            rs.player_read_chapter.clone().unwrap_or_default(),
                            rs.player_read_sequence.unwrap_or(rs.event_sequence),
                            true,
                        ),
                        _ => (rs.current_chapter.clone(), rs.event_sequence, false),
                    };

                script.current_chapter_key = restore_chapter.clone();
                script.current_event_process = restore_seq;

                // 读档续跑：若恢复位置来自「玩家阅读位置」且指向「AI 对话」事件，
                // **仅当**该事件的回复已完整生成（line_list 末尾是 assistant 行——
                // add_assistant_line 在回复最后一句 is_final 时写入）才前进一位跳过：
                // 此时玩家已看完全部回复，跳过不会跳剧情，且避免读档后重新调用 LLM
                // （数秒延迟、内容变化、网络失败中断剧本）。
                // 若回复未生成完（玩家读到一半就存档），则**不跳过**、正常重放重新生成
                // ——宁可重放也绝不跳剧情（这是 issue 的初衷）。
                if from_player_read {
                    let chapter_path = if restore_chapter.ends_with(".yaml") {
                        script.script_path.join("Chapters").join(&restore_chapter)
                    } else {
                        script
                            .script_path
                            .join("Chapters")
                            .join(format!("{}.yaml", restore_chapter))
                    };
                    if let Ok(content) = std::fs::read_to_string(&chapter_path) {
                        if let Ok(serde_yaml::Value::Mapping(map)) =
                            serde_yaml::from_str::<serde_yaml::Value>(&content)
                        {
                            if let Some(serde_yaml::Value::Sequence(events)) = map.get("events") {
                                if let Some(serde_yaml::Value::Mapping(ev)) =
                                    events.get(restore_seq.max(0) as usize)
                                {
                                    if ev.get("type").and_then(|v| v.as_str())
                                        == Some("ai_dialogue")
                                    {
                                        // 精确判断该 AI 对话的回复是否已完整生成：
                                        // 1) 新逻辑：vars 里有 __ai_reply_<seq>（生成成功时写入，
                                        //    连续 ai_dialogue 也不会误判为上一个事件的回复）；
                                        // 2) 兼容旧档（无该标记）：回退到 line_list 末尾 assistant 粗判断。
                                        let vars = serde_json::from_str::<serde_json::Map<
                                            String,
                                            serde_json::Value,
                                        >>(&rs.variable_info)
                                        .unwrap_or_default();
                                        let reply_complete = vars.contains_key(&format!(
                                            "__ai_reply_{}",
                                            restore_seq
                                        )) || {
                                            let gs = service.game_status.lock().await;
                                            gs.line_list.last().map_or(false, |l| {
                                                matches!(l.attribute(), LineAttribute::Assistant)
                                            })
                                        };
                                        if reply_complete {
                                            script.current_event_process += 1;
                                            tracing::info!(
                                                "[LoadSave] 恢复位置指向已完整生成的 AI 对话事件（#{}），前进一位跳过，不重新调用 LLM",
                                                restore_seq
                                            );
                                        } else {
                                            tracing::info!(
                                                "[LoadSave] 恢复位置指向 AI 对话事件（#{}）但回复未生成完，正常重放重新生成（不跳剧情）",
                                                restore_seq
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                script.vars = serde_json::from_str(&rs.variable_info).unwrap_or_default();
                let mut gs = service.game_status.lock().await;
                gs.script_status = Some(script);
                // 重置玩家阅读位置暂存：前端续跑后会随 `script:progress` 重新上报
                gs.player_read_chapter.clear();
                gs.player_read_seq = 0;
                // 递增剧本纪元：此后旧剧本任务的收尾（on_script_end）不会清理这份新恢复的状态
                gs.script_epoch = gs.script_epoch.wrapping_add(1);
                tracing::info!(
                    "[LoadSave] 已恢复剧本状态: {} 章节={} 事件={}（玩家位置: 章节={:?} 事件={:?}）",
                    rs.script_folder,
                    rs.current_chapter,
                    rs.event_sequence,
                    rs.player_read_chapter,
                    rs.player_read_sequence
                );
            } else {
                tracing::warn!(
                    "[LoadSave] 存档引用的剧本 '{}' 未在剧本目录中找到，跳过剧本恢复",
                    rs.script_folder
                );
            }
        }
    }

    // 10. 返回前端初始化数据
    let init = build_web_init_data(&service, &app).await?;
    tracing::info!(
        "[LoadSave] 读档完成: save_id={} 台词数={} active_script={:?} 总耗时 {:.0}ms",
        save_id,
        init.lines.len(),
        init.active_script,
        load_started.elapsed().as_millis()
    );
    Ok(init)
}

#[tauri::command]
pub async fn update_save(
    app: AppHandle,
    save_id: i32,
    screenshot_path: Option<String>,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let db = &state.db;

    let mut service = state.ai_service.lock().await;

    // 1. 校验存档存在
    SaveRepo::get_save_by_id(db, save_id)
        .await
        .map_err(|e| format!("查询存档失败: {}", e))?
        .ok_or_else(|| format!("存档 {} 不存在", save_id))?;

    // 复制截图到 screenshots 目录
    if let Some(ref path) = screenshot_path {
        let _ = save_screenshot_file(save_id, path).await;
    }

    let lines = service.game_status.lock().await.line_list.clone();

    // 2. 同步台词（智能 diff）
    SaveRepo::sync_lines(db, save_id, &lines)
        .await
        .map_err(|e| format!("同步台词失败: {}", e))?;

    // 3. 标记活跃存档
    service.game_status.lock().await.active_save_id = Some(save_id);

    // 4. 更新 GameStatus 快照
    let snapshot = service.game_status.lock().await.to_snapshot();
    let snapshot_json =
        serde_json::to_string(&snapshot).map_err(|e| format!("序列化状态失败: {}", e))?;
    SaveRepo::update_save_status(db, save_id, &snapshot_json)
        .await
        .map_err(|e| format!("保存状态失败: {}", e))?;

    // 5. 持久化 MemoryBank
    service
        .persist_memory_banks(save_id)
        .await
        .map_err(|e| format!("保存记忆库失败: {}", e))?;

    // 6. 持久化剧本状态
    {
        let gs = service.game_status.lock().await;
        if let Some(ref script_status) = gs.script_status {
            let vars_json = serde_json::to_string(&script_status.vars).unwrap_or_default();
            // 玩家阅读位置（前端上报）：与引擎位置并存，读档优先据此恢复
            let player_read_chapter = if gs.player_read_chapter.is_empty() {
                None
            } else {
                Some(gs.player_read_chapter.clone())
            };
            // 以 player_read_chapter 非空为「是否上报过」标志：首个内容事件的事件
            // 序号可能是 0（如剧本第一行旁白），不能因 seq>0 而漏存——否则读档
            // 时玩家位置为空，只能回退引擎位置。
            let player_read_sequence = if gs.player_read_chapter.is_empty() {
                None
            } else {
                Some(gs.player_read_seq.max(0))
            };
            let _ = SaveRepo::upsert_running_script(
                db,
                save_id,
                &script_status.folder_key,
                &vars_json,
                &script_status.current_chapter_key,
                script_status.current_event_process,
                player_read_chapter,
                player_read_sequence,
            )
            .await
            .map_err(|e| eprintln!("[SAVE_WARN] update_save: 保存剧本状态失败: {}", e));
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn delete_save(app: AppHandle, save_id: i32) -> Result<(), String> {
    let state = app.state::<AppState>();
    let db = &state.db;

    let service = state.ai_service.lock().await;

    // 1. 删除 MemoryBank
    SaveRepo::delete_memory_banks_by_save(db, save_id)
        .await
        .map_err(|e| format!("删除记忆库失败: {}", e))?;

    // 2. 删除 running_script 关联（若有）
    if let Ok(Some(save_model)) = SaveRepo::get_save_by_id(db, save_id).await {
        if let Some(rs_id) = save_model.running_script_id {
            let _ = SaveRepo::delete_running_script(db, rs_id).await;
        }
    }

    // 删除关联的截图文件
    let screenshot_path = super::data_dir()
        .join("screenshots")
        .join(format!("{}.png", save_id));
    if screenshot_path.exists() {
        let _ = std::fs::remove_file(screenshot_path);
    }

    // 3. 删除存档（级联删除关联的 line / line_perception）
    let deleted = SaveRepo::delete_save(db, save_id)
        .await
        .map_err(|e| format!("删除存档失败: {}", e))?;

    if !deleted {
        return Err(format!("存档 {} 不存在", save_id));
    }

    // 4. 若当前活跃存档是被删除的，清除标记
    if service.game_status.lock().await.active_save_id == Some(save_id) {
        service.game_status.lock().await.active_save_id = None;
    }

    Ok(())
}

#[tauri::command]
pub async fn update_save_title(app: AppHandle, save_id: i32, title: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let db = &state.db;
    SaveRepo::update_save_title(db, save_id, &title)
        .await
        .map_err(|e| format!("修改存档名称失败: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn save_screenshot(save_id: i32, screenshot_path: String) -> Result<(), String> {
    save_screenshot_file(save_id, &screenshot_path).await
}

/// 直接通过 HWND 截图主窗口（Windows）。
///
/// 跳过所有窗口枚举（`EnumWindows` / `Window::all()`），
/// 用 Tauri 拿到的原生 HWND 直接 GDI 截图 → 写入临时 PNG → 返回路径。
#[cfg(target_os = "windows")]
#[tauri::command]
pub async fn capture_main_window_screenshot(app: AppHandle) -> Result<String, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;

    let hwnd = window
        .hwnd()
        .map_err(|e| format!("获取窗口句柄失败: {}", e))?;

    // HWND.0 → *mut c_void → usize → u32（Windows 句柄是 32 位值）
    let id = hwnd.0 as usize as u32;

    let image = tauri_plugin_screenshots::windows::capture_own_window(id)?;

    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("lingchat_screenshot_{}.png", std::process::id()));
    image
        .save(&temp_path)
        .map_err(|e| format!("保存截图失败: {}", e))?;

    tracing::info!(
        "[capture_main_window_screenshot] Captured → {}",
        temp_path.display()
    );
    Ok(temp_path.to_string_lossy().to_string())
}

/// 非 Windows 平台的占位实现（该命令始终可注册，但在非 Windows 上返回错误）。
#[cfg(not(target_os = "windows"))]
#[tauri::command]
pub async fn capture_main_window_screenshot(_app: AppHandle) -> Result<String, String> {
    Err("capture_main_window_screenshot is only available on Windows".to_string())
}
