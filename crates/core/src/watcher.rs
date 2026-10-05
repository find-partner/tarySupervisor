//! 配置文件热加载（FR-2.6 / HLD RD-4）：
//! notify 监听配置文件 → 去抖 500 ms → 重新解析校验 → 与运行态 diff 串行应用。
//!
//! 校验失败：丢弃本次变更（沿用上一次有效配置），发 `ConfigInvalid` 事件提示。

use std::path::PathBuf;
use std::sync::{mpsc, Weak};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::service::ServiceManager;

/// 启动监视线程；返回的 watcher 必须保持存活（由 ServiceManager 持有）。
pub(super) fn spawn(path: PathBuf, weak: Weak<ServiceManager>) -> RecommendedWatcher {
    let (tx, rx) = mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        if res.is_ok() {
            // 只关心与配置文件相关的事件（同目录写临时文件也会触发 dir 事件，去抖后统一 reload）
            let _ = tx.send(());
        }
    })
    .expect("创建文件监听器失败");

    let watch_dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
        log::warn!("配置目录监听失败（热加载不可用）：{e}");
    }

    std::thread::Builder::new()
        .name("traysv-config-watch".into())
        .spawn(move || {
            let debounce = Duration::from_millis(500);
            loop {
                match rx.recv() {
                    Ok(()) => {}
                    Err(_) => return, // watcher 已释放
                }
                // 吸收去抖窗口内的后续事件
                loop {
                    match rx.recv_timeout(debounce) {
                        Ok(()) => continue,
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                let Some(mgr) = weak.upgrade() else { return };
                match mgr.reload_from_file() {
                    Ok(delta) => {
                        if !delta.is_empty() {
                            log::info!("配置热加载已应用：{delta:?}");
                            // ConfigChanged 事件由 reload 内部广播
                        }
                    }
                    Err(errors) => {
                        // 沿用上一次有效配置并提示（FR-2.6）
                        log::warn!("配置热加载校验失败，沿用上次配置：{errors:?}");
                        mgr.publish_config_invalid(errors);
                    }
                }
            }
        })
        .expect("启动配置监视线程失败");

    watcher
}
