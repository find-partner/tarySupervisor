//! 终端视图窗口（FR-6）：PTY 模式渲染后端 vte 解析出的屏幕帧；
//! 管道模式退化为纯文本日志流（FR-6.3）。关闭窗口不影响服务（FR-6.2）。
//! `term_interactive`（FR-6.4/6.5）服务支持键盘/粘贴输入 → write_stdin → PTY master。

use leptos::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use crate::bridge;
use crate::dto::{LogAppendedEv, RunDto, ServiceBriefOut, StateChangedEv, StyleDto, TermFrameEv, TermScreenDto};

fn style_css(s: &StyleDto) -> String {
    let mut css = String::new();
    if let Some(fg) = &s.fg {
        css.push_str(&format!("color:{};", crate::dto::color_css(fg)));
    }
    if let Some(bg) = &s.bg {
        css.push_str(&format!("background-color:{};", crate::dto::color_css(bg)));
    }
    if s.bold {
        css.push_str("font-weight:700;");
    }
    if s.italic {
        css.push_str("font-style:italic;");
    }
    if s.underline {
        css.push_str("text-decoration:underline;");
    }
    css
}

fn render_runs(runs: &[RunDto]) -> Vec<leptos::HtmlElement<html::Span>> {
    runs.iter()
        .map(|r| {
            let text = r.t.clone();
            let css = style_css(&r.s);
            view! {
                <span style=css>{text}</span>
            }
        })
        .collect()
}

/// 键盘事件 → 转发字节（SRS FR-6.4）：控制键按 ANSI 转义序列直传，
/// 行编辑交给子进程终端行规程；Cmd 组合键（浏览器/系统级）不拦截。
fn key_to_bytes(ev: &web_sys::KeyboardEvent) -> Option<Vec<u8>> {
    if ev.meta_key() {
        return None;
    }
    let key = ev.key();
    let bytes: Vec<u8> = match key.as_str() {
        "Enter" => b"\r".to_vec(),
        "Backspace" => b"\x7f".to_vec(),
        "Tab" => b"\t".to_vec(),
        "Escape" => b"\x1b".to_vec(),
        "ArrowUp" => b"\x1b[A".to_vec(),
        "ArrowDown" => b"\x1b[B".to_vec(),
        "ArrowRight" => b"\x1b[C".to_vec(),
        "ArrowLeft" => b"\x1b[D".to_vec(),
        "Home" => b"\x1b[H".to_vec(),
        "End" => b"\x1b[F".to_vec(),
        "Delete" => b"\x1b[3~".to_vec(),
        "PageUp" => b"\x1b[5~".to_vec(),
        "PageDown" => b"\x1b[6~".to_vec(),
        _ => {
            if ev.ctrl_key() && key.chars().count() == 1 {
                // Ctrl+字母 → 控制字符（Ctrl-C=0x03、Ctrl-D=0x04…）
                let c = key.chars().next().unwrap().to_ascii_lowercase();
                if c.is_ascii_lowercase() {
                    vec![c as u8 - b'a' + 1]
                } else if c == ' ' || c == '@' {
                    vec![0]
                } else {
                    return None;
                }
            } else if key.chars().count() == 1 {
                let mut s = String::new();
                if ev.alt_key() {
                    s.push('\x1b');
                }
                s.push(key.chars().next().unwrap());
                s.into_bytes()
            } else {
                return None;
            }
        }
    };
    Some(bytes)
}

#[component]
pub fn TermPage(id: String) -> impl IntoView {
    let (screen, set_screen) = create_signal::<Option<TermScreenDto>>(None);
    let (pipe_lines, set_pipe_lines) = create_signal::<Vec<(String, String)>>(Vec::new());
    let (state, set_state) = create_signal(String::from("…"));
    let (pid, set_pid) = create_signal::<Option<u32>>(None);
    let (name, set_name) = create_signal(String::new());
    let (is_pty, set_is_pty) = create_signal(true);
    // FR-6.4：交互能力（配置）与当前窗口读写开关（运行时）
    let (term_interactive, set_term_interactive) = create_signal(false);
    let (rw, set_rw) = create_signal(false);

    spawn_local(async move {
        let list: Vec<ServiceBriefOut> = bridge::invoke_t("list_services", None).await;
        let id_find = id.clone();
        if let Some(b) = list.iter().find(|b| b.config.id == id_find) {
            set_name.set(b.display_name().to_string());
            set_state.set(b.state.clone());
            set_pid.set(b.pid);
            set_is_pty.set(b.config.spawn_mode == "Pty");
            set_term_interactive.set(b.config.term_interactive && b.config.spawn_mode == "Pty");
            set_rw.set(b.config.term_interactive && b.config.spawn_mode == "Pty");
        }

        let id2 = id.clone();
        if is_pty.get_untracked() {
            let snap: Option<TermScreenDto> =
                bridge::invoke_t("term_snapshot", Some(bridge::invoke_args_id(&id2))).await;
            set_screen.set(snap);
            let id3 = id.clone();
            let _ = bridge
                ::listen("term-frame", move |payload| {
                    if let Ok(ev) = serde_wasm_bindgen::from_value::<TermFrameEv>(payload) {
                        if ev.id == id3 {
                            set_screen.set(Some(ev.screen));
                        }
                    }
                })
                .await;
        } else {
            // 管道模式：纯文本流
            let history: Vec<crate::dto::LogLine> = bridge::invoke_t(
                "get_log",
                Some(bridge::invoke_args_log(&id2, 0, 2000)),
            )
            .await;
            set_pipe_lines.set(
                history
                    .into_iter()
                    .map(|l| (l.stream, l.text))
                    .collect(),
            );
            let id3 = id.clone();
            let _ = bridge
                ::listen("log-appended", move |payload| {
                    if let Ok(ev) = serde_wasm_bindgen::from_value::<LogAppendedEv>(payload) {
                        if ev.id == id3 {
                            set_pipe_lines.update(|v| {
                                v.extend(ev.lines.into_iter().map(|l| (l.stream, l.text)));
                                if v.len() > 2000 {
                                    let drop = v.len() - 2000;
                                    v.drain(..drop);
                                }
                            });
                        }
                    }
                })
                .await;
        }

        let id4 = id.clone();
        let _ = bridge
            ::listen("service-state-changed", move |payload| {
                if let Ok(ev) = serde_wasm_bindgen::from_value::<StateChangedEv>(payload) {
                    if ev.id == id4 {
                        set_state.set(ev.info.state);
                        set_pid.set(ev.info.pid);
                    }
                }
            })
            .await;

        // ---- 交互输入（FR-6.4/6.5）：仅 term_interactive 服务挂监听 ----
        if !term_interactive.get_untracked() {
            return;
        }
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };

        let send = move |bytes: &[u8]| {
            if !rw.get_untracked() {
                return;
            }
            let alive = matches!(state.get_untracked().as_str(), "RUNNING" | "STARTING");
            if !alive {
                return;
            }
            let args = bridge::invoke_args_stdin(&id, bytes);
            spawn_local(async move {
                // 未运行/写入失败静默忽略（服务退出瞬间的输入无意义）
                let _ = bridge::invoke("write_stdin", Some(args)).await;
            });
        };

        let sender = send.clone();
        let key_handler: Closure<dyn FnMut(web_sys::KeyboardEvent)> =
            Closure::wrap(Box::new(move |ev: web_sys::KeyboardEvent| {
                let Some(bytes) = key_to_bytes(&ev) else { return };
                ev.prevent_default();
                sender(&bytes);
            }));
        let _ = doc.add_event_listener_with_callback(
            "keydown",
            key_handler.as_ref().unchecked_ref(),
        );
        key_handler.forget();

        let sender = send.clone();
        let paste_handler: Closure<dyn FnMut(web_sys::ClipboardEvent)> =
            Closure::wrap(Box::new(move |ev: web_sys::ClipboardEvent| {
                let Some(dt) = ev.clipboard_data() else { return };
                if let Ok(text) = dt.get_data("text") {
                    if !text.is_empty() {
                        ev.prevent_default();
                        sender(text.as_bytes());
                    }
                }
            }));
        let _ = doc.add_event_listener_with_callback(
            "paste",
            paste_handler.as_ref().unchecked_ref(),
        );
        paste_handler.forget();
    });

    view! {
        <div class="termwin">
            {move || if is_pty.get() {
                view! {
                    <pre class="termscreen">
                        {move || {
                            screen.get().map(|sc| {
                                sc.lines.into_iter().map(|row| {
                                    render_runs(&row)
                                }).collect::<Vec<_>>()
                            })
                        }}
                    </pre>
                }
            } else {
                view! {
                    <pre class="termscreen">
                        {move || {
                            pipe_lines.get().iter().map(|(stream, text)| {
                                let text = text.clone();
                                let css = if stream == "ERR" {
                                    "color:#f14c4c;".to_string()
                                } else {
                                    String::new()
                                };
                                view! { <span style=css>{text}</span> "\n" }
                            }).collect::<Vec<_>>()
                        }}
                    </pre>
                }
            }}
            <div class="termbar">
                <span>{move || name.get()}</span>
                <span>"·"</span>
                <span>{move || format!("{}{}", state.get(), pid.get().map(|p| format!(" (PID {p})")).unwrap_or_default())}</span>
                <span class="spacer"></span>
                {move || if term_interactive.get() {
                    view! {
                        <button
                            class:primary=move || rw.get()
                            on:click=move |_| set_rw.update(|v| *v = !*v)>
                            {move || if rw.get() { "⌨ 可写（点击切回只读）" } else { "🔒 只读（点击开启输入）" }}
                        </button>
                    }.into_view()
                } else {
                    view! { <span class="muted">"只读视图 · 关闭窗口不影响服务运行"</span> }.into_view()
                }}
            </div>
        </div>
    }
}
