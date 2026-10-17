//! 窗口管理：设置窗口（单实例）、每服务日志窗口与终端视图窗口。
//! 全部按需创建（label: settings / log:<id> / term:<id>），hash 路由到前端页面。

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::AppState;

/// 便携形态下把 WebView2 用户数据目录收进便携数据目录（否则 Windows 上 tauri
/// 默认写 `%LOCALAPPDATA%\{identifier}`，违反便携应用"不留痕"要求）。
/// 非便携返回 `None`，维持 tauri 默认（现有安装版行为不变）。
fn webview_data_dir() -> Option<std::path::PathBuf> {
    if !traysupervisor_core::is_portable() {
        return None;
    }
    Some(traysupervisor_core::config_dir().join("EBWebView"))
}

fn focus_or_create<F>(app: &AppHandle, label: &str, create: F)
where
    F: FnOnce(&str) -> Result<(), tauri::Error>,
{
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    } else if let Err(e) = create(label) {
        log::warn!("创建窗口 {label} 失败：{e}");
    }
}

pub fn open_settings(app: &AppHandle) {
    focus_or_create(app, "settings", |label: &str| {
        let mut builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html#/settings".into()))
            .title("TraySupervisor 设置")
            .inner_size(1000.0, 700.0)
            .min_inner_size(820.0, 560.0);
        if let Some(dir) = webview_data_dir() {
            builder = builder.data_directory(dir);
        }
        builder.build()?;
        Ok(())
    });
}

pub fn open_log(app: &AppHandle, svc_id: &str) {
    let label = format!("log:{svc_id}");
    let name = display_name(app, svc_id);
    let url = format!("index.html#/log/{svc_id}");
    focus_or_create(app, &label, move |label: &str| {
        let mut builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
            .title(format!("日志 — {name}"))
            .inner_size(920.0, 620.0);
        if let Some(dir) = webview_data_dir() {
            builder = builder.data_directory(dir);
        }
        builder.build()?;
        Ok(())
    });
}

pub fn open_term(app: &AppHandle, svc_id: &str) {
    let label = format!("term:{svc_id}");
    let name = display_name(app, svc_id);
    let url = format!("index.html#/term/{svc_id}");
    focus_or_create(app, &label, move |label: &str| {
        let mut builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
            .title(format!("终端视图 — {name}"))
            .inner_size(860.0, 520.0);
        if let Some(dir) = webview_data_dir() {
            builder = builder.data_directory(dir);
        }
        builder.build()?;
        Ok(())
    });
}

fn display_name(app: &AppHandle, svc_id: &str) -> String {
    app.try_state::<AppState>()
        .map(|s| {
            s.mgr
                .list()
                .into_iter()
                .find(|b| b.config.id == svc_id)
                .map(|b| b.config.display_name().to_string())
                .unwrap_or_else(|| svc_id.to_string())
        })
        .unwrap_or_else(|| svc_id.to_string())
}
