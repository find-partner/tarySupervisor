//! Tauri 壳入口：装配 core（ServiceManager）、托盘、事件桥接、命令与窗口。

mod bridge;
mod commands;
mod tray;
mod wins;

use std::sync::Arc;

use tauri::Manager;
use traysupervisor_core::config::ensure_default_config;
use traysupervisor_core::ServiceManager;

pub struct AppState {
    pub mgr: Arc<ServiceManager>,
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // FR-1.9 单实例：二次启动唤醒/聚焦设置窗口
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            wins::open_settings(app);
        }))
        .setup(|app| {
            // macOS 托盘应用：不显示 Dock 图标（等效 Info.plist 的 LSUIElement，开发与打包态一致）
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();

            // 配置目录与文件（FR-2.6 路径：~/.traysupervisor/config.ini）
            let config_path = traysupervisor_core::config_path();
            let cfg = match ensure_default_config(&config_path) {
                Ok(c) => c,
                Err(e) => {
                    // 配置存在但校验失败：提示并以上一次空配置启动（watcher 会在文件修复后热载）
                    log::error!("配置文件加载失败，以空配置启动：{e}");
                    traysupervisor_core::config::AppConfig::default()
                }
            };

            let mgr = ServiceManager::new(cfg, config_path)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            app.manage(AppState { mgr: mgr.clone() });

            // core 事件 → 前端广播（HLD §4.2）
            bridge::spawn(handle.clone(), mgr.clone());

            // 托盘 + 动态菜单（FR-1）
            tray::setup(&handle)?;

            // SIGTERM/SIGINT → 走退出流程（否则进程被直接终止，托管服务将成为孤儿）。
            // 仅 Unix：signal_hook 的 iterator 模块不存在于 Windows，退出走系统默认路径
            #[cfg(unix)]
            {
                let sig_app = handle.clone();
                std::thread::Builder::new()
                    .name("traysv-signal".into())
                    .spawn(move || {
                        use signal_hook::consts::{SIGINT, SIGTERM};
                        use signal_hook::iterator::Signals;
                        let mut signals = match Signals::new([SIGINT, SIGTERM]) {
                            Ok(s) => s,
                            Err(e) => {
                                log::warn!("信号注册失败（SIGTERM 时服务可能无法清理）：{e}");
                                return;
                            }
                        };
                        if let Some(sig) = signals.forever().next() {
                            log::info!("收到信号 {sig}，执行退出流程（停止全部服务）…");
                            sig_app.exit(0);
                        }
                    })
                    .ok();
            }

            log::info!("TraySupervisor 已启动（托盘常驻，无主窗口）");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_services,
            commands::upsert_service,
            commands::delete_service,
            commands::start_service,
            commands::stop_service,
            commands::restart_service,
            commands::get_log,
            commands::open_match_url,
            commands::get_env_resolved_preview,
            commands::parse_pasted_line,
            commands::get_platform_info,
            commands::reload_config,
            commands::term_snapshot,
            commands::write_stdin,
            commands::open_log_dir,
            commands::open_log_window,
            commands::open_term_window,
        ])
        .build(tauri::generate_context!())
        .expect("Tauri 构建失败")
        .run(|app, event| {
            use tauri::RunEvent;
            match event {
                // 无窗口常驻：关闭设置窗口不退出（托盘仍存活）
                RunEvent::ExitRequested { code, api, .. } => {
                    if code.is_none() {
                        api.prevent_exit();
                    } else if let Some(state) = app.try_state::<AppState>() {
                        // FR-3.7：应用退出前停止全部运行中服务（优雅 → 超时强杀）
                        log::info!("应用退出：停止全部运行中服务…");
                        state.mgr.shutdown_all();
                    }
                }
                _ => {}
            }
        });
}
