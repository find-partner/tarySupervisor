//! 只读终端视图窗口（FR-6）：PTY 模式渲染后端 vte 解析出的屏幕帧；
//! 管道模式退化为纯文本日志流（FR-6.3）。关闭窗口不影响服务（FR-6.2）。

use leptos::*;

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

#[component]
pub fn TermPage(id: String) -> impl IntoView {
    let (screen, set_screen) = create_signal::<Option<TermScreenDto>>(None);
    let (pipe_lines, set_pipe_lines) = create_signal::<Vec<(String, String)>>(Vec::new());
    let (state, set_state) = create_signal(String::from("…"));
    let (pid, set_pid) = create_signal::<Option<u32>>(None);
    let (name, set_name) = create_signal(String::new());
    let (is_pty, set_is_pty) = create_signal(true);

    spawn_local(async move {
        let list: Vec<ServiceBriefOut> = bridge::invoke_t("list_services", None).await;
        let id_find = id.clone();
        if let Some(b) = list.iter().find(|b| b.config.id == id_find) {
            set_name.set(b.display_name().to_string());
            set_state.set(b.state.clone());
            set_pid.set(b.pid);
            set_is_pty.set(b.config.spawn_mode == "Pty");
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
                <span class="muted">"只读视图 · 关闭窗口不影响服务运行"</span>
            </div>
        </div>
    }
}
