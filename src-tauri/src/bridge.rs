//! core 事件 → Tauri 前端广播（HLD §4.2 事件名映射）。

use std::sync::Arc;

use tauri::{AppHandle, Emitter};
use traysupervisor_core::events::CoreEvent;
use traysupervisor_core::ServiceManager;

pub fn spawn(app: AppHandle, mgr: Arc<ServiceManager>) {
    tauri::async_runtime::spawn(async move {
        let mut rx = mgr.events();
        loop {
            match rx.recv().await {
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Ok(ev) => {
                    let (name, payload): (&str, &CoreEvent) = match &ev {
                        CoreEvent::StateChanged { .. } => ("service-state-changed", &ev),
                        CoreEvent::LogAppended { .. } => ("log-appended", &ev),
                        CoreEvent::MatcherHit { .. } => ("matcher-hit", &ev),
                        CoreEvent::ConfigChanged { .. } => ("config-changed", &ev),
                        CoreEvent::ConfigInvalid { .. } => ("config-invalid", &ev),
                        CoreEvent::TermFrame { .. } => ("term-frame", &ev),
                    };
                    if let Err(e) = app.emit(name, payload) {
                        log::warn!("事件 {name} 发送失败：{e}");
                    }
                }
            }
        }
    });
}
