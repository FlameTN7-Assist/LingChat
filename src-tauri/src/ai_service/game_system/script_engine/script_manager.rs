//! ScriptManager — script discovery, lifecycle, and chapter orchestration.
//!
//! Replaces Python `ScriptManager` class. Scans the scripts directory for
//! `story_config.yaml` files, manages script start/run/complete, and provides
//! the user-input pause mechanism via oneshot channels.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::ai_service::game_system::script_engine::chapter::Chapter;
use crate::ai_service::game_system::script_engine::events::ScriptContext;
use crate::ai_service::game_system::script_engine::responses::{
    event_names::SCRIPT_END, ScriptEndPayload,
};
use crate::ai_service::message_system::events::emit;
use crate::ai_service::types::{AdventureConfig, LineAttributeExt, LineBase, ScriptStatus};
use crate::db::entities::line::LineAttribute;
use crate::db::entities::role::RoleType;
use crate::db::managers::role_repo::RoleRepo;
use crate::utils::prompt::{sys_prompt_builder, PromptOptions};

/// YAML structure for `story_config.yaml` top-level keys.
#[derive(serde::Deserialize, Default)]
struct StoryConfigRaw {
    script_name: Option<String>,
    intro_chapter: Option<String>,
    description: Option<String>,
    #[serde(default)]
    recommand_start: Option<String>,
    #[serde(default)]
    adventure: Option<AdventureConfig>,
    #[serde(default)]
    script_settings: Option<serde_json::Map<String, Value>>,
}

/// Central orchestrator for the script/story mode engine.
pub struct ScriptManager {
    /// All discovered scripts by name (folder_key or display name).
    pub all_scripts: HashMap<String, ScriptStatus>,
    /// Whether a script is currently running (shared so callers can read without lock).
    pub is_running: Arc<AtomicBool>,
    /// 当前运行中的剧本任务句柄（start_script/start_adventure/试玩 spawn 时登记）。
    /// 读档等场景需要「掐断旧任务」时据此 abort 并等待收尾，避免双任务竞争。
    pub current_run: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// 剧本运行纪元计数器：每次启动剧本/读档续跑递增，用于区分新旧剧本任务。
static NEXT_SCRIPT_EPOCH: AtomicU64 = AtomicU64::new(1);

/// 分配一个新的剧本运行纪元号。
pub fn next_script_epoch() -> u64 {
    NEXT_SCRIPT_EPOCH.fetch_add(1, Ordering::SeqCst)
}

impl ScriptManager {
    // ============================================================
    // Construction & script discovery
    // ============================================================

    /// Scan the scripts directory and build the script catalog.
    pub fn new(data_dir: &Path) -> Self {
        let mut manager = Self {
            all_scripts: HashMap::new(),
            is_running: Arc::new(AtomicBool::new(false)),
            current_run: Mutex::new(None),
        };
        manager.init_all_scripts(data_dir);
        manager
    }

    /// Scan `data_dir/game_data/scripts/` for all `story_config.yaml` files.
    fn init_all_scripts(&mut self, data_dir: &Path) {
        let scripts_dir = data_dir.join("game_data").join("scripts");
        if !scripts_dir.exists() {
            tracing::warn!("[ScriptManager] 剧本目录不存在: {:?}", scripts_dir);
            return;
        }

        // 1. Scan `character/<character>/<script>/` (two levels)
        let char_dir = scripts_dir.join("character");
        if char_dir.exists() {
            if let Ok(entries) = fs::read_dir(&char_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        if let Ok(sub_entries) = fs::read_dir(&path) {
                            for sub in sub_entries.flatten() {
                                let sub_path = sub.path();
                                if sub_path.is_dir() {
                                    self.try_load_script(&sub_path);
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. Scan `standalone/<script>/` (one level)
        let standalone_dir = scripts_dir.join("standalone");
        if standalone_dir.exists() {
            if let Ok(entries) = fs::read_dir(&standalone_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        self.try_load_script(&path);
                    }
                }
            }
        }

        // 3. Root-level scripts (backward compat)
        if let Ok(entries) = fs::read_dir(&scripts_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir()
                    && path
                        .file_name()
                        .map(|n| n != "character" && n != "standalone")
                        .unwrap_or(false)
                {
                    let config = path.join("story_config.yaml");
                    if config.exists() {
                        self.try_load_script(&path);
                    }
                }
            }
        }

        tracing::info!("[ScriptManager] 发现 {} 个剧本", self.all_scripts.len());
    }

    fn try_load_script(&mut self, script_path: &Path) {
        match Self::read_script_config(script_path) {
            Ok(script_status) => {
                let name = script_status.name.clone();
                self.all_scripts.insert(name, script_status);
            }
            Err(e) => {
                tracing::warn!("[ScriptManager] 跳过无效剧本目录 {:?}: {}", script_path, e);
            }
        }
    }

    /// Parse `story_config.yaml` from a script directory into a `ScriptStatus`.
    pub fn read_script_config(script_path: &Path) -> Result<ScriptStatus> {
        let config_path = script_path.join("story_config.yaml");
        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("无法读取剧本配置: {:?}", config_path))?;

        let raw: StoryConfigRaw = serde_yaml::from_str(&content)
            .with_context(|| format!("无法解析剧本配置: {:?}", config_path))?;

        let folder_key = script_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let name = raw.script_name.unwrap_or_else(|| folder_key.clone());
        let intro_chapter = raw.intro_chapter.unwrap_or_else(|| "main".to_string());
        let description = raw.description.unwrap_or_default();
        let adventure = raw.adventure.unwrap_or_default();
        let settings = raw.script_settings.unwrap_or_default();

        Ok(ScriptStatus {
            folder_key,
            name,
            description,
            intro_chapter,
            settings,
            script_path: script_path.to_path_buf(),
            recommand_start: raw.recommand_start.unwrap_or_default(),
            adventure,
            running_client_id: None,
            current_chapter_key: String::new(),
            current_event_process: 0,
            vars: serde_json::Map::new(),
        })
    }

    // ============================================================
    // Script listing
    // ============================================================

    pub fn get_script_list(&self) -> Vec<String> {
        self.all_scripts.keys().cloned().collect()
    }

    pub fn get_standalone_script_list(&self) -> Vec<String> {
        self.all_scripts
            .iter()
            .filter(|(_, s)| !s.adventure.is_adventure)
            .map(|(k, _)| k.clone())
            .collect()
    }

    pub fn get_script(&self, name: &str) -> Option<&ScriptStatus> {
        self.all_scripts.get(name)
    }

    // ============================================================
    // Script lifecycle
    // ============================================================

    /// Main entry point: initialize and run a script by name.
    /// This is a long-running async operation — it awaits user input inside.
    pub async fn start_script(&self, name: &str, ctx: &mut ScriptContext<'_>) -> Result<()> {
        let script = self
            .all_scripts
            .get(name)
            .ok_or_else(|| anyhow!("剧本不存在: '{}'", name))?
            .clone();

        self.is_running.store(true, Ordering::SeqCst);

        Self::run_to_completion(&script, ctx, &self.is_running).await
    }

    /// Execute a script from start to finish without needing `&self`.
    /// This is the entry point for the API layer, which holds script data
    /// independently and builds its own `ScriptContext`.
    pub async fn execute_script(
        script: &ScriptStatus,
        ctx: &mut ScriptContext<'_>,
        is_running: &AtomicBool,
    ) -> Result<()> {
        is_running.store(true, Ordering::SeqCst);
        Self::run_to_completion(script, ctx, is_running).await
    }

    /// Init → run → teardown, with teardown guaranteed on the error path.
    ///
    /// The previous code chained `?` between the three steps, so any failure
    /// skipped `on_script_end`: `is_running` stayed `true`, `script_status`
    /// stayed `Some(..)`, `script:end` was never emitted and the frontend froze
    /// in story mode with no way out. Callers only logged the error.
    async fn run_to_completion(
        script: &ScriptStatus,
        ctx: &mut ScriptContext<'_>,
        is_running: &AtomicBool,
    ) -> Result<()> {
        // Deliberately sequential rather than an `async { .. }.await` block:
        // an async block would capture `ctx: &mut _` by move, leaving it
        // unusable for the teardown below.
        let mut outcome = Self::init_script(script, ctx).await;
        if outcome.is_ok() {
            outcome = Self::run_script(ctx).await;
        }

        if let Err(e) = &outcome {
            tracing::error!("[ScriptManager] 剧本 '{}' 执行失败: {:#}", script.name, e);
            // Surface it before teardown: emits `ai:error` + `status:reset`,
            // both of which the frontend already listens for.
            crate::ai_service::message_system::events::emit_error(ctx.app, e);
        }

        // A failed run must not be recorded as completed — that would unlock
        // follow-up adventures the player never actually finished.
        let completed = outcome.is_ok();
        if let Err(e) = Self::on_script_end(ctx, is_running, completed).await {
            tracing::error!("[ScriptManager] 剧本收尾失败: {:#}", e);
        }

        outcome
    }

    /// Initialize a script: register its roles, set script_status, load player info.
    pub async fn init_script(script: &ScriptStatus, ctx: &mut ScriptContext<'_>) -> Result<()> {
        // Set script_status on GameStatus
        {
            let mut gs = ctx.game_status.lock().await;
            // 若 load_save 已恢复同一剧本的进度（current_chapter_key 非空），
            // 保留其章节/事件/变量，使 run_script 能从存档章节续跑；仅合并剧本元数据。
            let has_resume = gs
                .script_status
                .as_ref()
                .map(|s| s.folder_key == script.folder_key && !s.current_chapter_key.is_empty())
                .unwrap_or(false);
            if has_resume {
                if let Some(ref mut ss) = gs.script_status {
                    ss.name = script.name.clone();
                    ss.description = script.description.clone();
                    ss.intro_chapter = script.intro_chapter.clone();
                    ss.settings = script.settings.clone();
                    ss.script_path = script.script_path.clone();
                    ss.recommand_start = script.recommand_start.clone();
                    ss.adventure = script.adventure.clone();
                    tracing::info!(
                        "[ScriptManager] 续跑恢复: 剧本 '{}' 从章节 '{}' 事件 {} 继续",
                        script.name,
                        ss.current_chapter_key,
                        ss.current_event_process
                    );
                }
            } else {
                gs.script_status = Some(script.clone());
            }
            // 记录本次运行的纪元号（on_script_end 据此判断是否清理剧本状态）
            gs.script_epoch = ctx.run_epoch;
            // 清空玩家阅读位置暂存：本次运行的阅读位置由前端随 script:progress 重新上报，
            // 避免残留上一轮剧本/读档前的位置，导致存档写入错误章节。
            gs.player_read_chapter.clear();
            gs.player_read_seq = 0;
        }

        // Load player info from script settings
        if let Some(user_name) = script.settings.get("user_name").and_then(|v| v.as_str()) {
            if !user_name.is_empty() {
                ctx.game_status.lock().await.player.user_name = user_name.to_string();
            }
        }
        if let Some(user_subtitle) = script
            .settings
            .get("user_subtitle")
            .and_then(|v| v.as_str())
        {
            ctx.game_status.lock().await.player.user_subtitle = user_subtitle.to_string();
        }

        // Register script roles from characters/ subdirectory (if exists)
        Self::register_script_roles(script, ctx).await?;

        tracing::info!("[ScriptManager] 剧本 '{}' 初始化完成", script.name);
        Ok(())
    }

    /// Register script-specific NPC roles into DB and load them.
    pub async fn register_script_roles(
        script: &ScriptStatus,
        ctx: &mut ScriptContext<'_>,
    ) -> Result<()> {
        let characters_dir = script.script_path.join("characters");
        if !characters_dir.exists() {
            return Ok(());
        }

        let entries = fs::read_dir(&characters_dir)
            .with_context(|| format!("无法读取角色目录: {:?}", characters_dir))?;

        // Get RoleManager for mutating role state
        // We need to work with game_status directly
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let role_folder = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            // Read settings.yml BEFORE the existence check: the lookup key comes
            // out of it. Previously the check used `role_folder` while creation
            // passed `settings.script_role_key`, so whenever settings.yml omitted
            // that field the two disagreed — `find_or_create_role` skipped its
            // own lookup (it needs Some(script_role_key)) and inserted a fresh
            // duplicate row on *every* script start, while `character: <folder>`
            // in the YAML could never resolve to any of them.
            let settings_path = path.join("settings.yml");
            if !settings_path.exists() {
                tracing::warn!("[ScriptManager] 角色缺少 settings.yml: {:?}", settings_path);
                continue;
            }

            let content = fs::read_to_string(&settings_path)
                .with_context(|| format!("无法读取角色设定: {:?}", settings_path))?;

            let settings: crate::ai_service::types::CharacterSettings =
                serde_yaml::from_str(&content)
                    .with_context(|| format!("无法解析角色设定: {:?}", settings_path))?;

            // 剧本角色必须显式带 script_role_key —— 它在数据库里把「剧本 NPC」与
            // game_data/characters/ 的主角色区分开。缺了它就不能加载（上游硬要求），
            // 不能再用目录名回退替代，否则数据库角色记录会混乱。
            // 官方剧本均无 characters/ 目录，故不受影响；编辑器创建/导入角色时已强制写入。
            let role_key = match settings
                .script_role_key
                .as_deref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
            {
                Some(k) => k.to_string(),
                None => {
                    tracing::warn!(
                        "[ScriptManager] 角色 {:?} 缺少 script_role_key，跳过加载（剧本 NPC 必须显式声明该字段）",
                        role_folder
                    );
                    continue;
                }
            };

            // 取剧本角色 ID：优先复用数据库中已存在的同 key（script_key + script_role_key）
            // 角色，不存在则新建。**已存在角色也要继续走「加载 + 补 SYSTEM 人设行」流程**——
            // 否则剧本后续运行时该角色只有 DB 记录、运行时 line_list 里没有系统人设台词，
            // role.memory 无人设、LLM 不按【情绪】格式输出，AI 回复整段被丢弃
            // （表现：思考提示后「AI 正要吐字、剧本跳到了下一个事件」+ 人设丢失告警）。
            let path_key = script.path_key();
            let role_id = match RoleRepo::get_role_by_script_keys(ctx.db, &path_key, &role_key).await?
            {
                Some(existing) => {
                    tracing::info!(
                        "[ScriptManager] 角色已存在: script={}, role_key={}, id={}",
                        path_key,
                        role_key,
                        existing.id
                    );
                    existing.id
                }
                None => {
                    // Register role in DB via RoleRepo
                    let role_id = RoleRepo::find_or_create_role(
                        ctx.db,
                        &settings.ai_name,
                        RoleType::Npc,
                        Some(&path_key),
                        Some(&role_key),
                        Some(&role_folder),
                    )
                    .await?;

                    tracing::info!(
                        "[ScriptManager] 注册剧本角色: {} (id={}, script={}, role_key={})",
                        settings.ai_name,
                        role_id,
                        path_key,
                        role_key
                    );
                    role_id
                }
            };

            // Load the role into RoleManager（已存在角色同样要加载，立绘/名字/记忆才可用）
            let _ = ctx
                .game_status
                .lock()
                .await
                .get_role(ctx.db, role_id)
                .await?;

            // Add system prompt line for this role（新老角色一视同仁：缺人设行就补）
            let prompt = settings.system_prompt.clone().unwrap_or_default();
            let prompt_options = PromptOptions {
                output_sec_lang: true,
                no_emotion_limit: true,
            };
            if !prompt.is_empty() {
                let ai_prompt = sys_prompt_builder(
                    &ctx.game_status.lock().await.player.user_name,
                    &settings.ai_name,
                    &prompt,
                    settings.system_prompt_example.as_deref(),
                    settings.system_prompt_example_old.as_deref(),
                    prompt_options,
                );
                // 若该角色已有 system 人设台词（如读档恢复时已补建），跳过避免重复注入
                let already_has_system = ctx
                    .game_status
                    .lock()
                    .await
                    .line_list
                    .iter()
                    .any(|l| {
                        matches!(l.attribute(), LineAttribute::System)
                            && l.base.sender_role_id == Some(role_id)
                    });
                if !already_has_system {
                    let sys_line = LineBase {
                        content: ai_prompt,
                        attribute: LineAttributeExt(LineAttribute::System),
                        sender_role_id: Some(role_id),
                        display_name: Some(settings.ai_name.clone()),
                        ..Default::default()
                    };
                    ctx.game_status
                        .lock()
                        .await
                        .add_line(ctx.db, sys_line)
                        .await?;
                }
            }
        }

        Ok(())
    }

    /// The main chapter loop: load chapters and run them until "end".
    pub async fn run_script(ctx: &mut ScriptContext<'_>) -> Result<()> {
        let script = ctx
            .game_status
            .lock()
            .await
            .script_status
            .as_ref()
            .ok_or_else(|| anyhow!("ScriptStatus 未设置"))?
            .clone();

        // 若已恢复存档进度（current_chapter_key 非空），从存档章节续跑；
        // 否则按剧本 intro_chapter 正常开始。
        let mut next_chapter = if !script.current_chapter_key.is_empty() {
            script.current_chapter_key.clone()
        } else {
            script.intro_chapter.clone()
        };

        // Resolve "Intro/intro" style paths → find the actual yaml file
        let chapters_dir = script.script_path.join("Chapters");

        while next_chapter != "end" {
            let chapter_path = if next_chapter.ends_with(".yaml") {
                chapters_dir.join(&next_chapter)
            } else {
                chapters_dir.join(format!("{}.yaml", next_chapter))
            };

            let content = fs::read_to_string(&chapter_path)
                .with_context(|| format!("无法读取章节文件: {:?}", chapter_path))?;

            let chapter_config: Value = serde_yaml::from_str(&content)
                .with_context(|| format!("无法解析章节文件: {:?}", chapter_path))?;

            let script_ref = ctx
                .game_status
                .lock()
                .await
                .script_status
                .as_ref()
                .ok_or_else(|| anyhow!("ScriptStatus 丢失"))?
                .clone();

            let mut chapter = Chapter::new(next_chapter.clone(), chapter_config, &script_ref);

            // Update tracking fields
            if let Some(ref mut ss) = ctx.game_status.lock().await.script_status {
                ss.current_chapter_key = next_chapter.clone();
                ss.current_event_process = 0;
            }

            next_chapter = chapter.run(ctx).await?;
        }

        Ok(())
    }

    /// Cleanup after script ends: emit script_end event, clear script_status,
    /// mark adventures complete.
    /// Tear down the running script.
    ///
    /// `completed` records whether the run finished normally. Only a completed
    /// run is added to `completed_scripts` (which gates adventure unlocks);
    /// a run that ended in an error still gets fully torn down so the UI is
    /// released, but is not credited to the player.
    pub async fn on_script_end(
        ctx: &mut ScriptContext<'_>,
        is_running: &AtomicBool,
        completed: bool,
    ) -> Result<()> {
        tracing::info!("[ScriptManager] 剧本结束 (completed={})", completed);

        // Emit script_end event. This is what releases the frontend from story
        // mode, so it must fire on the error path too — but flagged so the
        // client does not mark the adventure as completed.
        let _ = emit(ctx.app, SCRIPT_END, &ScriptEndPayload { completed });

        // Extract data under one lock, then mutate under a second lock.
        // tokio::sync::Mutex is NOT reentrant — nesting lock().await deadlocks.
        // 仅当当前 script_status 属于本次运行（纪元一致）时才记录/清理；
        // 读档恢复的新剧本状态不会被旧任务的收尾误清。
        let (folder, is_adventure, epoch_matches) = {
            let gs = ctx.game_status.lock().await;
            let epoch_matches = gs.script_epoch == ctx.run_epoch;
            match gs.script_status.as_ref() {
                Some(ss) => (
                    Some(ss.path_key()),
                    ss.adventure.is_adventure,
                    epoch_matches,
                ),
                None => (None, false, epoch_matches),
            }
        };

        // Now re-acquire the lock and do all writes in one critical section
        if epoch_matches {
            let mut gs = ctx.game_status.lock().await;
            if let Some(folder) = folder {
                if completed {
                    gs.completed_scripts.insert(folder.clone());
                    if is_adventure {
                        tracing::info!("[ScriptManager] 羁绊冒险完成: {}", folder);
                    }
                } else {
                    tracing::warn!("[ScriptManager] 剧本未正常结束，不记为已完成: {}", folder);
                }
            }
            gs.script_status = None;
            tracing::info!("[ScriptManager] 剧本状态已清除");
        } else {
            tracing::info!(
                "[ScriptManager] 本次运行({})已结束，但当前剧本状态属于其他运行({})，跳过清理",
                ctx.run_epoch,
                ctx.game_status.lock().await.script_epoch
            );
        }

        is_running.store(false, Ordering::SeqCst);

        Ok(())
    }

    // ============================================================
    // Adventure management
    // ============================================================

    pub fn get_character_adventures(&self, character_folder: &str) -> Vec<&ScriptStatus> {
        self.all_scripts
            .values()
            .filter(|s| {
                s.adventure.is_adventure && s.adventure.bound_character_folder == character_folder
            })
            .collect()
    }

    pub fn get_all_adventures(&self) -> Vec<&ScriptStatus> {
        self.all_scripts
            .values()
            .filter(|s| s.adventure.is_adventure)
            .collect()
    }

    pub fn get_assets_dir(&self, script_name: Option<&str>) -> PathBuf {
        match script_name.and_then(|n| self.all_scripts.get(n)) {
            Some(script) => script.script_path.join("Assets"),
            None => PathBuf::from(""),
        }
    }
}
