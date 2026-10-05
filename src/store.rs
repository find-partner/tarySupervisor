//! 前端共享服务数据源（FR-1.10 / HLD §3.6）：
//! 每个窗口（设置/日志/终端都是独立 WebView 实例）内以全局信号持有
//! `list_services` 快照——与托盘菜单同一后端数据源（`ServiceManager::list()`）；
//! 订阅 `service-state-changed` 就地更新对应条目、`config-changed`/`config-invalid`
//! 全量刷新。设置页服务列表与编辑页操作栏（FR-2.11）共用此信号，
//! 保证界面状态与托盘菜单一致。
//!
//! 注意：`ensure()` 须在组件作用域内首次调用（create_rw_signal 需要响应式属主）。

use std::sync::OnceLock;

use leptos::*;

use crate::bridge;
use crate::dto::{ServiceBriefOut, StateChangedEv};

static STORE: OnceLock<RwSignal<Vec<ServiceBriefOut>>> = OnceLock::new();

/// 取共享列表信号（首次调用时初始化并挂事件监听）
pub fn ensure() -> RwSignal<Vec<ServiceBriefOut>> {
    *STORE.get_or_init(|| {
        let list = create_rw_signal(Vec::new());

        // 全量刷新（初始 + 配置变更）
        spawn_local(async move {
            refresh().await;
        });

        // 状态变更：就地更新对应条目（与托盘 apply_state 同源事件）
        spawn_local(async move {
            let _ = bridge
                ::listen("service-state-changed", move |payload| {
                    if let Ok(ev) = serde_wasm_bindgen::from_value::<StateChangedEv>(payload) {
                        if let Some(sig) = STORE.get() {
                            sig.update(|v| {
                                if let Some(b) = v.iter_mut().find(|b| b.config.id == ev.id) {
                                    b.state = ev.info.state.clone();
                                    b.pid = ev.info.pid;
                                }
                            });
                        }
                    }
                })
                .await;
            // 服务集合变化 / 热加载 → 全量刷新（FR-2.6 / FR-1.10）
            let _ = bridge
                ::listen("config-changed", move |_| {
                    spawn_local(async move { refresh().await });
                })
                .await;
            let _ = bridge
                ::listen("config-invalid", move |_| {
                    spawn_local(async move { refresh().await });
                })
                .await;
        });

        list
    })
}

/// 重新拉取 list_services 快照（托盘菜单同款数据源）
pub async fn refresh() {
    let Some(sig) = STORE.get() else { return };
    let l: Vec<ServiceBriefOut> = bridge::invoke_t("list_services", None).await;
    sig.set(l);
}
