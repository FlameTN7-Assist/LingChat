//! 平台适配。
//!
//! 这里的两项适配都必须在窗口 / WebView 创建**之前**完成，因此无法走 Tauri 的
//! 设置体系，只能在启动早期直接操作进程环境变量：
//! - Windows：WebView2 颜色配置文件，需绕过 AppHandle 直接读 store 文件；
//! - Linux：WebKitGTK 在 NVIDIA 专有驱动下的渲染兼容，WebKitWebProcess 子进程
//!   会继承父进程环境变量，故必须早于 GTK / WebKit 初始化。
//!
//! 本模块原先位于 crate 根（`lib.rs`）。

/// 读取 settings.json 中的「HDR 模式」开关（仅 Windows）。
///
/// 必须在 WebView2 环境创建（`Builder::build()`）之前调用——此时 `AppHandle` 尚不存在，
/// 只能直接解析 store 文件。store 位于 `%APPDATA%\<identifier>\settings.json`
/// （tauri-plugin-store 的 flat 点号键）。文件缺失/解析失败一律视为「未开启」。
#[cfg(target_os = "windows")]
fn read_hdr_mode_enabled(identifier: &str) -> bool {
    use serde_json::Value;

    let Some(appdata) = std::env::var("APPDATA").ok() else {
        return false;
    };
    let path = std::path::Path::new(&appdata)
        .join(identifier)
        .join(ling_chat_main::config::STORE_FILE);

    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<Value>(&content) else {
        return false;
    };
    json.get(ling_chat_main::config::keys::HDR_MODE_ENABLED)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Windows：设置 WebView2 颜色配置文件（强制使用线性 sRGB）。
///
/// 未开启 HDR 模式时，把 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 设为
/// `--force-color-profile=scrgb-linear`，避免 WebView2 在 HDR 屏上做二次色彩映射。
/// 非 Windows 平台为空实现，以便 `run()` 调用点无需 `#[cfg]`。
#[cfg(target_os = "windows")]
pub fn apply_webview2_color_profile(identifier: &str) {
    if !read_hdr_mode_enabled(identifier) {
        // `std::env::set_var` 在 Rust 2024 起被标记为 unsafe；此处仍在 2021 edition，
        // 但为将来迁移保留 `#[allow(deprecated)]`。调用点在 WebView2 创建之前，
        // 尚无其它线程读取环境变量。
        #[allow(deprecated)]
        unsafe {
            std::env::set_var(
                "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                "--force-color-profile=scrgb-linear",
            );
        }
    }
}

/// 非 Windows 平台无颜色配置文件设置需求。
#[cfg(not(target_os = "windows"))]
pub fn apply_webview2_color_profile(_identifier: &str) {}

/// Linux：WebKitGTK 在 NVIDIA 专有驱动下的渲染兼容处理。
///
/// WebKitGTK 的 DMA-BUF renderer 会在窗口 surface 上启用 Wayland explicit sync，
/// 却不总是先设置 acquire point；严格校验该规则的 compositor（Hyprland / KWin）
/// 会直接以协议错误掐断连接（`Gdk-Message: Error 71`），X11 下则表现为白屏。
///
/// 判定与所需的环境变量交给 `webkit2gtk-nvidia-quirk`：只有 NVIDIA 专有驱动
/// （vendor 0x10de 且驱动名为 nvidia）才动手，AMD / Intel / nouveau 一律不设；
/// 用户已自行设置过相关环境变量时也不覆盖。
#[cfg(target_os = "linux")]
pub fn apply_webkit_nvidia_quirks() {
    use webkit2gtk_nvidia_quirk::{
        ApplyWorkaroundOptions, WorkaroundKind, apply_workaround_with_options, needs_workaround,
    };

    // 逃生口：自动判定不适用（例如 PRIME 笔记本判定偏差）时允许用户关闭
    if webkit_quirk_disabled(std::env::var("LINGCHAT_WEBKIT_QUIRK").ok().as_deref()) {
        tracing::info!("[WebKit] LINGCHAT_WEBKIT_QUIRK=0，跳过 NVIDIA 渲染兼容处理");
        return;
    }

    // 先只读探测并落日志：用户反馈渲染问题时可直接看到判定依据
    let kind = needs_workaround();
    tracing::info!("[WebKit] NVIDIA 渲染兼容判定: {kind:?}");
    if kind == WorkaroundKind::None {
        return;
    }

    apply_workaround_with_options(ApplyWorkaroundOptions::default());
    // 记录最终环境变量状态：用户已自行设置过时 crate 不会覆盖，这里如实反映结果
    tracing::info!(
        "[WebKit] 渲染兼容处理完成，最终环境变量: WEBKIT_DISABLE_DMABUF_RENDERER={:?}, __NV_DISABLE_EXPLICIT_SYNC={:?}",
        std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").ok(),
        std::env::var("__NV_DISABLE_EXPLICIT_SYNC").ok(),
    );
}

/// 解析 `LINGCHAT_WEBKIT_QUIRK` 逃生口：仅显式 `0` 表示关闭自动兼容处理。
#[cfg(target_os = "linux")]
fn webkit_quirk_disabled(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("0"))
}

/// 非 Linux 平台无 WebKitGTK 渲染兼容需求。
#[cfg(not(target_os = "linux"))]
pub fn apply_webkit_nvidia_quirks() {}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::webkit_quirk_disabled;

    #[test]
    fn only_explicit_zero_disables_the_quirk() {
        assert!(webkit_quirk_disabled(Some("0")));
        assert!(webkit_quirk_disabled(Some(" 0 ")));
        assert!(!webkit_quirk_disabled(Some("1")));
        assert!(!webkit_quirk_disabled(Some("")));
        assert!(!webkit_quirk_disabled(None));
    }
}
