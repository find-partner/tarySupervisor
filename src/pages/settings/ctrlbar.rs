//! 编辑页服务操作栏（SRS FR-2.11 / HLD §3.6）：编辑模式标签栏上方提供与托盘
//! 菜单对等的能力——状态显示（色点 + 状态 + PID）、启动/停止/重启（按状态
//! 启用/禁用）、查看日志、终端视图。
//! 数据取共享 store（FR-1.10，与托盘/列表同源），动作调 start/stop/restart_service
//! 与 open_log_window/open_term_window；状态随 service-state-changed 实时刷新。

use leptos::*;

use crate::bridge;
use crate::store;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Start,
    Stop,
    Restart,
}

impl Action {
    fn cmd(self) -> &'static str {
        match self {
            Action::Start => "start_service",
            Action::Stop => "stop_service",
            Action::Restart => "restart_service",
        }
    }
}

pub fn ctrl_bar(
    selected_id: RwSignal<Option<String>>,
    op_err: RwSignal<String>,
) -> impl IntoView {
    let list = store::ensure();

    let brief = move || {
        let id = selected_id.get();
        list.get().into_iter().find(|b| Some(&b.config.id) == id.as_ref())
    };

    let do_action = move |id: String, action: Action| {
        spawn_local(async move {
            // 失败（如「当前为 RUNNING，无需启动」）在栏内提示；成功状态经事件刷新
            match bridge::invoke_unit_res(action.cmd(), Some(bridge::invoke_args_id(&id))).await {
                Ok(()) => op_err.set(String::new()),
                Err(e) => op_err.set(e),
            }
        });
    };
    let open_win = move |id: String, cmd: &'static str| {
        spawn_local(async move {
            let _ = bridge::invoke(cmd, Some(bridge::invoke_args_id(&id))).await;
        });
    };

    view! {
        <div class="ctrlbar">
            {move || match brief() {
                None => view! { <span class="muted">"服务未加载"</span> }.into_view(),
                Some(b) => {
                    let cls = b.state_class();
                    let id = b.config.id.clone();
                    let name = b.display_name().to_string();
                    let st = b.state.clone();
                    let pid = b.pid;
                    let state_text = match pid {
                        Some(p) => format!("{st}（PID {p}）"),
                        None => st.clone(),
                    };
                    let can_start = matches!(st.as_str(), "STOPPED" | "EXITED" | "FATAL" | "BACKOFF");
                    let can_stop = matches!(st.as_str(), "STARTING" | "RUNNING" | "STOPPING" | "BACKOFF");
                    let can_restart = st != "STOPPING";

                    let id_start = id.clone();
                    let id_stop = id.clone();
                    let id_restart = id.clone();
                    let id_log = id.clone();
                    let id_term = id;
                    view! {
                        <span class=format!("dot {cls}")></span>
                        <b>{name}</b>
                        <span class="muted">{state_text}</span>
                        <button disabled=!can_start on:click=move |_| do_action(id_start.clone(), Action::Start)>"启动"</button>
                        <button disabled=!can_stop on:click=move |_| do_action(id_stop.clone(), Action::Stop)>"停止"</button>
                        <button disabled=!can_restart on:click=move |_| do_action(id_restart.clone(), Action::Restart)>"重启"</button>
                        <span class="spacer"></span>
                        <button on:click=move |_| open_win(id_log.clone(), "open_log_window")>"查看日志"</button>
                        <button on:click=move |_| open_win(id_term.clone(), "open_term_window")>"终端视图"</button>
                    }.into_view()
                }
            }}
        </div>
        {move || {
            let e = op_err.get();
            if e.is_empty() {
                view! { <div></div> }.into_view()
            } else {
                view! { <div class="errors" style="margin:0 0 8px;">{e}</div> }.into_view()
            }
        }}
    }
}
