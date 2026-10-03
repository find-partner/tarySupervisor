//! 窗口管理：设置窗口（单实例）、每服务日志窗口与终端视图窗口。
//! 全部按需创建（label: settings / log:<id> / term:<id>），hash 路由到前端页面。

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::AppState;

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
        WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html#/settings".into()))
            .title("TraySupervisor 设置")
            .inner_size(1000.0, 700.0)
            .min_inner_size(820.0, 560.0)
            .build()?;
        Ok(())
    });
}

pub fn open_log(app: &AppHandle, svc_id: &str) {
    let label = format!("log:{svc_id}");
    let name = display_name(app, svc_id);
    let url = format!("index.html#/log/{svc_id}");
    focus_or_create(app, &label, move |label: &str| {
        WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
            .title(format!("日志 — {name}"))
            .inner_size(920.0, 620.0)
            .build()?;
        Ok(())
    });
}

pub fn open_term(app: &AppHandle, svc_id: &str) {
    let label = format!("term:{svc_id}");
    let name = display_name(app, svc_id);
    let url = format!("index.html#/term/{svc_id}");
    focus_or_create(app, &label, move |label: &str| {
        WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
            .title(format!("终端视图 — {name}"))
            .inner_size(860.0, 520.0)
            .build()?;
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
