//! 日志查看窗口（FR-4.4/4.6）：自动滚动到底（上滚暂停、回底恢复）、复制、
//! 清空视图、打开日志目录；窗口关闭不影响后端捕获。

use leptos::*;
use wasm_bindgen::JsValue;
use wasm_bindgen::JsCast;

use crate::bridge;
use crate::dto::{LogAppendedEv, LogLine, ServiceBriefOut, StateChangedEv};

#[component]
pub fn LogPage(id: String) -> impl IntoView {
    let (lines, set_lines) = create_signal::<Vec<LogLine>>(Vec::new());
    let (state, set_state) = create_signal(String::from("…"));
    let (pid, set_pid) = create_signal::<Option<u32>>(None);
    let (name, set_name) = create_signal(String::new());
    let (autoscroll, set_autoscroll) = create_signal(true);
    let container_ref = create_node_ref::<html::Div>();
    let id_dir = id.clone(); // 「打开日志目录」按钮用

    // 历史 + 增量
    spawn_local(async move {
        let list: Vec<ServiceBriefOut> = bridge::invoke_t("list_services", None).await;
        let id_find = id.clone();
        if let Some(b) = list.iter().find(|b| b.config.id == id_find) {
            set_name.set(b.display_name().to_string());
            set_state.set(b.state.clone());
            set_pid.set(b.pid);
        }
        let history: Vec<LogLine> =
            bridge::invoke_t("get_log", Some(bridge::invoke_args_log(&id, 0, 5000))).await;
        set_lines.set(history);

        let id2 = id.clone();
        let _ = bridge
            ::listen("log-appended", move |payload| {
                if let Ok(ev) = serde_wasm_bindgen::from_value::<LogAppendedEv>(payload) {
                    if ev.id == id2 {
                        set_lines.update(|v| {
                            v.extend(ev.lines);
                            if v.len() > 5000 {
                                let drop = v.len() - 5000;
                                v.drain(..drop);
                            }
                        });
                    }
                }
            })
            .await;
        let _ = bridge
            ::listen("service-state-changed", move |payload| {
                if let Ok(ev) = serde_wasm_bindgen::from_value::<StateChangedEv>(payload) {
                    if ev.id == id {
                        set_state.set(ev.info.state);
                        set_pid.set(ev.info.pid);
                    }
                }
            })
            .await;
    });

    // 自动滚动：内容变化时若处于跟随状态则滚到底部（FR-4.4）
    create_effect(move |_| {
        let _ = lines.get();
        if !autoscroll.get() {
            return;
        }
        if let Some(el) = container_ref.get() {
            el.set_scroll_top(el.scroll_height());
        }
    });

    let on_scroll = move |e: web_sys::Event| {
        let el = e.target().expect("scroll target").unchecked_into::<web_sys::Element>();
        let at_bottom = el.scroll_height() - el.scroll_top() - el.client_height() < 30;
        set_autoscroll.set(at_bottom);
    };

    let toggle_follow = move |_| set_autoscroll.update(|v| *v = !*v);
    let clear_view = move |_| set_lines.set(Vec::new());
    let copy_all = move |_| {
        let text = lines
            .get()
            .iter()
            .map(|l| format!("[{}] [{}] {}", l.ts, l.stream, l.text))
            .collect::<Vec<_>>()
            .join("\n");
        if let Ok(clipboard) =
            js_sys::Reflect::get(&web_sys::window().expect("window").navigator(), &JsValue::from_str("clipboard"))
        {
            if let Ok(write_text) = js_sys::Reflect::get(&clipboard, &JsValue::from_str("writeText")) {
                if let Ok(cb) = write_text.dyn_into::<js_sys::Function>() {
                    let _ = cb.call1(&clipboard, &JsValue::from_str(&text));
                }
            }
        }
    };
    let open_dir = move |_| {
        let id = id_dir.clone();
        spawn_local(async move {
            let _ = bridge::invoke_unit_res("open_log_dir", Some(bridge::invoke_args_id(&id))).await;
        });
    };

    let time_of = |ts: &str| -> String {
        // "YYYY-MM-DD HH:MM:SS.mmm" → "HH:MM:SS"
        ts.get(11..19).unwrap_or(ts).to_string()
    };

    view! {
        <div class="logwin">
            <div class="toolbar">
                <b>{move || name.get()}</b>
                <span class="dot" class:muted=move || state.get() == "STOPPED"
                    class:ok=move || state.get() == "RUNNING"
                    class:warn=move || matches!(state.get().as_str(), "STARTING" | "STOPPING" | "BACKOFF")
                    class:bad=move || matches!(state.get().as_str(), "EXITED" | "FATAL")></span>
                <span class="muted">
                    {move || format!("{}{}", state.get(), pid.get().map(|p| format!(" · PID {p}")).unwrap_or_default())}
                </span>
                <span class="spacer"></span>
                <button on:click=toggle_follow>
                    {move || if autoscroll.get() { "跟随滚动：开" } else { "跟随滚动：关" }}
                </button>
                <button on:click=copy_all>"复制全部"</button>
                <button on:click=clear_view>"清空视图"</button>
                <button on:click=open_dir>"打开日志目录"</button>
            </div>
            <div class="logview" node_ref=container_ref on:scroll=on_scroll>
                {move || {
                    let data = lines.get();
                    data.iter().map(|l| {
                        let l = l.clone();
                        let stream = l.stream.clone();
                        let t = time_of(&l.ts);
                        let is_err = l.stream == "ERR";
                        let text = l.text.clone();
                        view! {
                            <span class="ln">
                                <span class="t">"[" {t} "] "</span>
                                <span class="tag">"[" {stream} "] "</span>
                                <span class=move || if is_err { "err" } else { "" }>{text}</span>
                            </span>
                        }
                    }).collect::<Vec<_>>()
                }}
            </div>
        </div>
    }
}
