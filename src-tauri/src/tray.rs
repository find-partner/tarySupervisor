//! 托盘菜单（FR-1 / HLD §3.5）：
//! - 事件订阅 → 300ms 防抖合并 → 全量重建菜单（D-7）；
//! - 菜单 ID 稳定编码（FR-1.7）：svc:<id>:start|stop|restart|log|term、match:<id>:<hash>、app:*；
//! - 状态色圆点用 IconMenuItem 承载（FR-1.5），tooltip = 「运行中 n / 共 m 个服务」（FR-1.1）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::menu::{IconMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};
use traysupervisor_core::events::SvcState;
use traysupervisor_core::ServiceManager;

use crate::wins;
use crate::AppState;

/// 托盘句柄（重建菜单用）
pub struct TrayHandle {
    pub tray: tauri::tray::TrayIcon,
}

pub fn setup(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let state = app.state::<AppState>();
    let mgr = state.mgr.clone();

    // 初始菜单
    let menu = build_menu(app, &mgr)?;
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip(tooltip_text(&mgr))
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| handle_menu_event(app, event.id().as_ref()));
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let tray = builder.build(app)?;
    app.manage(TrayHandle { tray });

    // 事件 → 防抖重建（FR-1.6：≤300ms 内多次变更合并为一次重建）
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut rx = mgr.events();
        let needs = Arc::new(AtomicBool::new(false));
        let inflight = Arc::new(AtomicBool::new(false));
        loop {
            if rx.recv().await.is_err() {
                break;
            }
            needs.store(true, Ordering::SeqCst);
            if inflight.swap(true, Ordering::SeqCst) {
                continue;
            }
            let (needs2, inflight2, app3, mgr2) =
                (needs.clone(), inflight.clone(), app2.clone(), mgr.clone());
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                loop {
                    needs2.store(false, Ordering::SeqCst);
                    rebuild(&app3, &mgr2);
                    if !needs2.load(Ordering::SeqCst) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                inflight2.store(false, Ordering::SeqCst);
            });
        }
    });

    Ok(())
}

fn rebuild(app: &AppHandle, mgr: &Arc<ServiceManager>) {
    let Some(tray) = app.try_state::<TrayHandle>() else { return };
    match build_menu(app, mgr) {
        Ok(menu) => {
            let _ = tray.tray.set_menu(Some(menu));
            let _ = tray.tray.set_tooltip(Some(tooltip_text(mgr)));
            log::debug!("托盘菜单已重建");
        }
        Err(e) => log::warn!("托盘菜单重建失败：{e}"),
    }
}

fn tooltip_text(mgr: &ServiceManager) -> String {
    format!("TraySupervisor：运行中 {} / 共 {} 个服务", mgr.running_count(), mgr.total_count())
}

/// 状态圆点 8×8 RGBA 图标
fn dot_icon(state: SvcState) -> tauri::image::Image<'static> {
    let (r, g, b) = state.color_rgb();
    let size = 8usize;
    let mut rgba = Vec::with_capacity(size * size * 4);
    let c = (size as f32 - 1.0) / 2.0;
    for y in 0..size {
        for x in 0..size {
            let dist = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let alpha = if dist <= c + 0.5 { 255u8 } else { 0u8 };
            rgba.extend_from_slice(&[r, g, b, alpha]);
        }
    }
    tauri::image::Image::new_owned(rgba, size as u32, size as u32)
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, mgr: &Arc<ServiceManager>) -> Result<Menu<R>, Box<dyn std::error::Error>> {
    let menu = Menu::new(app)?;

    for brief in mgr.list() {
        let id = brief.config.id.clone();
        let display = brief.config.display_name().to_string();
        let st = brief.state;

        let submenu = Submenu::with_id(app, format!("svc-sub:{id}"), display, true)?;

        // ① 状态行（只读）
        let status_text = match brief.pid {
            Some(pid) => format!("● {} (PID {pid})", st.as_str()),
            None => format!("● {}", st.as_str()),
        };
        let status = IconMenuItem::with_id(
            app,
            format!("svc:{id}:status"),
            status_text,
            false,
            Some(dot_icon(st)),
            None::<&str>,
        )?;
        submenu.append(&status)?;
        submenu.append(&PredefinedMenuItem::separator(app)?)?;

        // ② 启动/停止/重启（按状态启用/禁用）
        let can_start = matches!(st, SvcState::Stopped | SvcState::Exited | SvcState::Fatal | SvcState::Backoff);
        let can_stop = matches!(st, SvcState::Starting | SvcState::Running | SvcState::Stopping | SvcState::Backoff);
        let can_restart = !matches!(st, SvcState::Stopping);
        for (action, label, enabled) in [
            ("start", "启动", can_start),
            ("stop", "停止", can_stop),
            ("restart", "重启", can_restart),
        ] {
            let item = MenuItem::with_id(
                app,
                format!("svc:{id}:{action}"),
                label,
                enabled,
                None::<&str>,
            )?;
            submenu.append(&item)?;
        }

        // ③④ 查看日志 / 终端视图
        submenu.append(&PredefinedMenuItem::separator(app)?)?;
        let log_item = MenuItem::with_id(app, format!("svc:{id}:log"), "查看日志", true, None::<&str>)?;
        submenu.append(&log_item)?;
        let term_label = if brief.config.spawn_mode == traysupervisor_core::config::SpawnMode::Pty {
            "终端视图"
        } else {
            "终端视图（纯文本）"
        };
        let term_item = MenuItem::with_id(app, format!("svc:{id}:term"), term_label, true, None::<&str>)?;
        submenu.append(&term_item)?;

        // ⑤ Matcher 快捷项（动态生成，FR-5）
        if !brief.match_items.is_empty() {
            submenu.append(&PredefinedMenuItem::separator(app)?)?;
            for item in &brief.match_items {
                let mi = MenuItem::with_id(
                    app,
                    format!("match:{}:{}", id, item.hash),
                    &item.label,
                    item.url.is_some(), // 非 URL 项禁用（FR-5.6）
                    None::<&str>,
                )?;
                submenu.append(&mi)?;
            }
        }

        menu.append(&submenu)?;
    }

    if !mgr.list().is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }

    // 设置… / 退出（FR-1.3；全部启动/全部停止为 P1，MVP 不含）
    let settings = MenuItem::with_id(app, "app:settings", "设置…", true, None::<&str>)?;
    menu.append(&settings)?;
    let quit = MenuItem::with_id(app, "app:quit", "退出", true, None::<&str>)?;
    menu.append(&quit)?;

    Ok(menu)
}

fn handle_menu_event(app: &AppHandle, event_id: &str) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let mgr = state.mgr.clone();

    let parts: Vec<&str> = event_id.splitn(3, ':').collect();
    match (parts.first().copied(), parts.get(1).copied(), parts.get(2).copied()) {
        (Some("app"), Some("settings"), _) => wins::open_settings(app),
        (Some("app"), Some("quit"), _) => quit_flow(app, &mgr),
        (Some("svc"), Some(id), Some(action)) => match action {
            "start" => {
                if let Err(e) = mgr.start(id) {
                    log::warn!("{e}");
                }
                rebuild_now(app);
            }
            "stop" => {
                if let Err(e) = mgr.stop(id) {
                    log::warn!("{e}");
                }
                rebuild_now(app);
            }
            "restart" => {
                if let Err(e) = mgr.restart(id) {
                    log::warn!("{e}");
                }
                rebuild_now(app);
            }
            "log" => wins::open_log(app, id),
            "term" => wins::open_term(app, id),
            _ => {}
        },
        (Some("match"), Some(svc_id), Some(rest)) => {
            if let Ok(hash) = rest.parse::<u64>() {
                // open_match_url 命令的复用逻辑（FR-5.6 校验 + opener）
                match mgr.match_url(svc_id, hash) {
                    Some(url) if traysupervisor_core::matcher::is_safe_url(&url) => {
                        use tauri_plugin_opener::OpenerExt;
                        if let Err(e) = app.opener().open_url(&url, None::<&str>) {
                            log::warn!("打开链接失败：{e}");
                        }
                    }
                    Some(url) => log::warn!("拒绝非 http/https 链接：{url}"),
                    None => log::warn!("快捷项不存在：{svc_id}#{hash}"),
                }
            }
        }
        _ => {}
    }
}

/// 立即重建（状态类操作后的即时反馈；其余走 300ms 防抖）
fn rebuild_now(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let Some(tray) = app.try_state::<TrayHandle>() else { return };
    if let Ok(menu) = build_menu(app, &state.mgr) {
        let _ = tray.tray.set_menu(Some(menu));
        let _ = tray.tray.set_tooltip(Some(tooltip_text(&state.mgr)));
        log::debug!("托盘菜单已重建（操作即时反馈）");
    }
}

/// FR-1.8：若有运行中服务，弹确认对话框（列出将被停止的服务）；确认后逐个停止再退出
fn quit_flow(app: &AppHandle, mgr: &Arc<ServiceManager>) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let app = app.clone();

    let running: Vec<String> = mgr
        .list()
        .into_iter()
        .filter(|b| b.state.is_alive())
        .map(|b| b.config.display_name().to_string())
        .collect();

    if running.is_empty() {
        app.exit(0);
        return;
    }
    let msg = format!(
        "以下服务正在运行，退出后将按停止流程（优雅停止 → 超时强杀）逐个停止：\n\n  · {}\n\n确认退出？",
        running.join("\n  · ")
    );
    app.dialog()
        .message(msg)
        .title("退出确认")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom("退出".to_string(), "取消".to_string()))
        .show(move |confirmed| {
            if confirmed {
                // ExitRequested(code=None) 已被拦截；显式 exit(code=Some) 触发 FR-3.7 停止流程
                app.exit(0);
            }
        });
}
