//! 托盘菜单（FR-1 / HLD §3.5 / D-7）：
//! - **持久菜单 + 菜单项就地更新**：StateChanged / MatcherHit 只对既有菜单项
//!   `set_text` / `set_icon` / `set_enabled`、在子菜单内增删快捷项，不替换菜单
//!   对象——菜单展开期间状态变化**不会导致菜单消失**（FR-1.6；旧实现每次
//!   `set_menu()` 全量重建，菜单展开时会被系统直接关闭）；
//! - 仅服务集合变化（ConfigChanged）才全量重建；
//! - 菜单 ID 稳定编码（FR-1.7）：svc:<id>:start|stop|restart|log|term、
//!   match:<id>:<hash>、app:*；
//! - 状态色圆点用 IconMenuItem 承载（FR-1.5），tooltip =「运行中 n / 共 m 个
//!   服务」（FR-1.1）；数据源与设置页一致（ServiceManager::list()，FR-1.10）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tauri::menu::{IconMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};
use traysupervisor_core::events::{CoreEvent, MatchItem, StateInfo, SvcState};
use traysupervisor_core::ServiceManager;

use crate::wins;
use crate::AppState;

/// 每服务的持久菜单项句柄（FR-1.6 就地更新）
struct SvcItems {
    status: IconMenuItem<Wry>,
    start: MenuItem<Wry>,
    stop: MenuItem<Wry>,
    restart: MenuItem<Wry>,
    /// 快捷项前的分隔线（快捷项清空时一并移除）
    matcher_sep: Option<PredefinedMenuItem<Wry>>,
    /// Matcher 快捷项（MatcherHit 时整组移除重建，FR-5.5 重启清空）
    matcher: Vec<MenuItem<Wry>>,
}

struct TrayInner {
    menu: Menu<Wry>,
    submenus: HashMap<String, Submenu<Wry>>,
    items: HashMap<String, SvcItems>,
}

/// 托盘句柄（持久菜单 + 每服务项句柄）
pub struct TrayHandle {
    tray: tauri::tray::TrayIcon,
    inner: Mutex<TrayInner>,
}

pub fn setup(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let state = app.state::<AppState>();
    let mgr = state.mgr.clone();

    let inner = build_menu(app, &mgr)?;
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip(tooltip_text(&mgr))
        .menu(&inner.menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| handle_menu_event(app, event.id().as_ref()));
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let tray = builder.build(app)?;
    app.manage(TrayHandle { tray, inner: Mutex::new(inner) });

    // 事件 → 分类分流（FR-1.6 / D-7）：状态与 Matcher 就地更新，配置变更全量重建。
    // 与设置页共用同一事件流与数据源（FR-1.10）；LogAppended / TermFrame 等高频
    // 事件与菜单无关，直接跳过。
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut rx = mgr.events();
        loop {
            let ev = match rx.recv().await {
                Ok(ev) => ev,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            match ev {
                CoreEvent::StateChanged { id, info } => apply_state(&app2, &mgr, &id, &info),
                CoreEvent::MatcherHit { svc_id, items } => apply_matchers(&app2, &svc_id, &items),
                CoreEvent::ConfigChanged { .. } => rebuild_all(&app2, &mgr),
                _ => {}
            }
        }
    });

    Ok(())
}

fn tooltip_text(mgr: &ServiceManager) -> String {
    format!("TraySupervisor：运行中 {} / 共 {} 个服务", mgr.running_count(), mgr.total_count())
}

/// 状态行文案（FR-1.4 ①）
fn status_text(st: SvcState, pid: Option<u32>) -> String {
    match pid {
        Some(pid) => format!("● {} (PID {pid})", st.as_str()),
        None => format!("● {}", st.as_str()),
    }
}

/// 启动/停止/重启的启用位（FR-1.4 ②）
fn action_flags(st: SvcState) -> (bool, bool, bool) {
    let can_start = matches!(st, SvcState::Stopped | SvcState::Exited | SvcState::Fatal | SvcState::Backoff);
    let can_stop = matches!(st, SvcState::Starting | SvcState::Running | SvcState::Stopping | SvcState::Backoff);
    let can_restart = !matches!(st, SvcState::Stopping);
    (can_start, can_stop, can_restart)
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

/// 全量构建菜单（setup 与服务集合变化时的重建共用），返回持久句柄。
fn build_menu(app: &AppHandle, mgr: &Arc<ServiceManager>) -> Result<TrayInner, Box<dyn std::error::Error>> {
    let menu = Menu::new(app)?;
    let mut submenus = HashMap::new();
    let mut items = HashMap::new();

    for brief in mgr.list() {
        let id = brief.config.id.clone();
        let display = brief.config.display_name().to_string();
        let st = brief.state;

        let submenu = Submenu::with_id(app, format!("svc-sub:{id}"), display, true)?;

        // ① 状态行（只读）
        let status = IconMenuItem::with_id(
            app,
            format!("svc:{id}:status"),
            status_text(st, brief.pid),
            false,
            Some(dot_icon(st)),
            None::<&str>,
        )?;
        submenu.append(&status)?;
        submenu.append(&PredefinedMenuItem::separator(app)?)?;

        // ② 启动/停止/重启（按状态启用/禁用）
        let (can_start, can_stop, can_restart) = action_flags(st);
        let start = MenuItem::with_id(app, format!("svc:{id}:start"), "启动", can_start, None::<&str>)?;
        let stop = MenuItem::with_id(app, format!("svc:{id}:stop"), "停止", can_stop, None::<&str>)?;
        let restart = MenuItem::with_id(app, format!("svc:{id}:restart"), "重启", can_restart, None::<&str>)?;
        submenu.append(&start)?;
        submenu.append(&stop)?;
        submenu.append(&restart)?;

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
        let mut svc = SvcItems { status, start, stop, restart, matcher_sep: None, matcher: vec![] };
        if !brief.match_items.is_empty() {
            let sep = PredefinedMenuItem::separator(app)?;
            submenu.append(&sep)?;
            svc.matcher_sep = Some(sep);
            for item in &brief.match_items {
                let mi = matcher_item(app, &id, item)?;
                submenu.append(&mi)?;
                svc.matcher.push(mi);
            }
        }

        menu.append(&submenu)?;
        submenus.insert(id.clone(), submenu);
        items.insert(id, svc);
    }

    if !mgr.list().is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }

    // 设置… / 退出（FR-1.3；全部启动/全部停止为 P1，MVP 不含）
    let settings = MenuItem::with_id(app, "app:settings", "设置…", true, None::<&str>)?;
    menu.append(&settings)?;
    let quit = MenuItem::with_id(app, "app:quit", "退出", true, None::<&str>)?;
    menu.append(&quit)?;

    Ok(TrayInner { menu, submenus, items })
}

fn matcher_item(app: &AppHandle, svc_id: &str, item: &MatchItem) -> Result<MenuItem<Wry>, tauri::Error> {
    MenuItem::with_id(
        app,
        format!("match:{svc_id}:{}", item.hash),
        &item.label,
        item.url.is_some(), // 非 URL 项禁用（FR-5.6）
        None::<&str>,
    )
}

/// StateChanged → 就地更新（FR-1.6：不替换菜单对象，展开中的菜单不消失）
fn apply_state(app: &AppHandle, mgr: &Arc<ServiceManager>, id: &str, info: &StateInfo) {
    let Some(handle) = app.try_state::<TrayHandle>() else { return };
    let missing = {
        let inner = handle.inner.lock().unwrap();
        match inner.items.get(id) {
            Some(svc) => {
                let _ = svc.status.set_text(status_text(info.state, info.pid));
                let _ = svc.status.set_icon(Some(dot_icon(info.state)));
                let (can_start, can_stop, can_restart) = action_flags(info.state);
                let _ = svc.start.set_enabled(can_start);
                let _ = svc.stop.set_enabled(can_stop);
                let _ = svc.restart.set_enabled(can_restart);
                log::debug!("托盘菜单项已就地更新：{id} → {}", info.state.as_str());
                false
            }
            // 持久表中没有该服务（错过重建等异常时序）→ 回退全量重建
            None => true,
        }
    };
    if missing {
        rebuild_all(app, mgr);
        return;
    }
    let _ = handle.tray.set_tooltip(Some(tooltip_text(mgr)));
}

/// MatcherHit → 子菜单内整组移除旧快捷项、追加新项（D-7）。
/// MatcherHit 仅在快捷项集合实际变化时发出（matcher 去重 + LRU）；重启清空
/// （FR-5.5）时 hits 为空 → 移除全组与分隔线。
fn apply_matchers(app: &AppHandle, svc_id: &str, hits: &[MatchItem]) {
    let Some(handle) = app.try_state::<TrayHandle>() else { return };
    let mut inner = handle.inner.lock().unwrap();
    // 先解构出字段（经 Deref 的整对象借用无法同时可变/不可变借用两个字段）
    let TrayInner { submenus, items, .. } = &mut *inner;
    let Some(sub) = submenus.get(svc_id) else { return };
    let Some(svc) = items.get_mut(svc_id) else { return };
    for old in svc.matcher.drain(..) {
        let _ = sub.remove(&old);
    }
    if let Some(sep) = svc.matcher_sep.take() {
        let _ = sub.remove(&sep);
    }
    if hits.is_empty() {
        return;
    }
    if let Ok(sep) = PredefinedMenuItem::separator(app) {
        if sub.append(&sep).is_ok() {
            svc.matcher_sep = Some(sep);
        }
    }
    for item in hits {
        match matcher_item(app, svc_id, item) {
            Ok(mi) => {
                if sub.append(&mi).is_ok() {
                    svc.matcher.push(mi);
                }
            }
            Err(e) => log::warn!("快捷项创建失败：{e}"),
        }
    }
}

/// ConfigChanged（服务集合变化）→ 全量重建并替换菜单（FR-1.6 的唯一重建路径）。
/// 此时菜单若在展开中会被系统关闭——可接受：该事件仅在用户编辑配置时发生。
fn rebuild_all(app: &AppHandle, mgr: &Arc<ServiceManager>) {
    let Some(handle) = app.try_state::<TrayHandle>() else { return };
    match build_menu(app, mgr) {
        Ok(new) => {
            let mut guard = handle.inner.lock().unwrap();
            let old_menu = std::mem::replace(&mut guard.menu, new.menu);
            guard.submenus = new.submenus;
            guard.items = new.items;
            let _ = handle.tray.set_menu(Some(guard.menu.clone()));
            let _ = handle.tray.set_tooltip(Some(tooltip_text(mgr)));
            drop(old_menu); // 旧菜单在替换完成后释放
            log::debug!("托盘菜单已重建（服务集合变化）");
        }
        Err(e) => log::warn!("托盘菜单重建失败：{e}"),
    }
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
            }
            "stop" => {
                if let Err(e) = mgr.stop(id) {
                    log::warn!("{e}");
                }
            }
            "restart" => {
                if let Err(e) = mgr.restart(id) {
                    log::warn!("{e}");
                }
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
