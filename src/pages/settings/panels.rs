//! 表格式标签页：环境变量（FR-2.3）、日志（轮转策略）、快捷项（FR-5 Matcher）。

use leptos::*;

use crate::dto::{EnvEntry, MatcherConfig};

use super::form::{set_form_value, FormState};

pub fn env_panel(
    form: RwSignal<FormState>,
    dirty: RwSignal<Vec<bool>>,
    on_preview: impl Fn(()) + Copy + 'static,
) -> impl IntoView {
    let mark1 = move || dirty.update(|d| d[1] = true);
    let add_row = move |_| {
        form.update(|f| f.env.push(EnvEntry { key: String::new(), value: String::new(), expand: false }));
        mark1();
    };
    view! {
        <div class="hint" style="margin:0 0 8px;">"按书写顺序注入；勾选「替换」后值中的 ${VAR} 先查本表在前条目、再查系统环境（FR-2.3）"</div>
        <table class="kv">
            <thead>
                <tr><th>"Key"</th><th>"Value"</th><th>"替换"</th><th></th></tr>
            </thead>
            <tbody>
                {move || {
                    form.get().env.iter().enumerate().map(|(i, _)| {
                        let rm = move |_| {
                            form.update(|f| { f.env.remove(i); });
                            mark1();
                        };
                        view! {
                            <tr>
                                <td><input type="text"
                                    prop:value=move || form.get().env.get(i).map(|e| e.key.clone()).unwrap_or_default()
                                    on:input=move |e| {
                                        let v = event_target_value(&e);
                                        form.update(|f| { if let Some(en) = f.env.get_mut(i) { en.key = v; } });
                                        mark1();
                                    } /></td>
                                <td><input type="text"
                                    prop:value=move || form.get().env.get(i).map(|e| e.value.clone()).unwrap_or_default()
                                    on:input=move |e| {
                                        let v = event_target_value(&e);
                                        form.update(|f| { if let Some(en) = f.env.get_mut(i) { en.value = v; } });
                                        mark1();
                                    } /></td>
                                <td class="center"><input type="checkbox"
                                    prop:checked=move || form.get().env.get(i).map(|e| e.expand).unwrap_or(false)
                                    on:change=move |e| {
                                        let v = event_target_checked(&e);
                                        form.update(|f| { if let Some(en) = f.env.get_mut(i) { en.expand = v; } });
                                        mark1();
                                    } /></td>
                                <td class="center"><button class="danger" on:click=rm>"删除"</button></td>
                            </tr>
                        }
                    }).collect::<Vec<_>>()
                }}
            </tbody>
        </table>
        <div style="margin-top:6px">
            <button on:click=add_row>"添加一行"</button>
            <button on:click=move |_| on_preview(())>"预览展开结果"</button>
        </div>
    }
}

/// GUI 空路径时的内置默认路径前缀（保存时后端 expand_tilde 展开为绝对路径；
/// 与 crates/core default_log_path / CONFIG-SPEC §1 配置目录约定保持同步）
const DEFAULT_LOG_DIR: &str = "~/.traysupervisor/logs";

pub fn log_panel(form: RwSignal<FormState>, dirty: RwSignal<Vec<bool>>) -> impl IntoView {
    let mark2 = move || dirty.update(|d| d[2] = true);
    view! {
        <div class="field-grid">
            <label>"写入文件"</label>
            <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                <input type="checkbox" prop:checked=move || form.get().log_enabled
                    on:change=move |e| {
                        let v = event_target_checked(&e);
                        set_form_value(form, move |f| {
                            f.log_enabled = v;
                            // 勾选启用且路径为空 → 直接生成内置默认路径（清空后保存则由后端兜底生成）
                            if v && f.log_path.trim().is_empty() && !f.id.trim().is_empty() {
                                f.log_path = format!("{DEFAULT_LOG_DIR}/{}.log", f.id.trim());
                            }
                        });
                        mark2();
                    } />
                "启用文件日志（不启用 = 仅内存环形缓冲）"
            </label>
            <label>"文件路径"</label>
            <input type="text" placeholder="留空保存时自动生成 ~/.traysupervisor/logs/<服务ID>.log"
                prop:value=move || form.get().log_path.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.log_path = v);
                    mark2();
                } />
            <label>"单文件上限"</label>
            <input type="text" placeholder="如 10MB、512KB、1GB（0 = 不轮转）"
                prop:value=move || form.get().log_max_bytes.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.log_max_bytes = v);
                    mark2();
                } />
            <span class="hint">"支持 KB/MB/GB 单位（大小写不敏感）或纯字节数；0 = 不轮转；超出后按 .1 .2 … 分块"</span>
            <label>"保留份数"</label>
            <input type="number" min="0" max="100" prop:value=move || form.get().log_max_files.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.log_max_files = v);
                    mark2();
                } />
            <label>"gzip 压缩旧分块"</label>
            <input type="checkbox" prop:checked=move || form.get().log_compress
                on:change=move |e| {
                    let v = event_target_checked(&e);
                    set_form_value(form, move |f| f.log_compress = v);
                    mark2();
                } />
        </div>
    }
}

pub fn matcher_panel(form: RwSignal<FormState>, dirty: RwSignal<Vec<bool>>) -> impl IntoView {
    let mark3 = move || dirty.update(|d| d[3] = true);
    let add_m = move |_| {
        form.update(|f| f.matchers.push(MatcherConfig {
            pattern: String::new(), template: "🌐 打开 {0}".into(), max_items: 5,
        }));
        mark3();
    };
    view! {
        <div class="hint" style="margin:0 0 8px;">"日志正则 → 托盘快捷项；新建服务默认预填内置 HTTP URL 规则（FR-5.3），删除后不会隐式启用"</div>
        <table class="kv">
            <thead>
                <tr><th>"Pattern（Rust regex）"</th><th>"模板（{0} {1} 引用捕获组）"</th><th>"上限"</th><th></th></tr>
            </thead>
            <tbody>
                {move || {
                    form.get().matchers.iter().enumerate().map(|(i, _)| {
                        let rm = move |_| {
                            form.update(|f| { f.matchers.remove(i); });
                            mark3();
                        };
                        view! {
                            <tr>
                                <td><input type="text"
                                    prop:value=move || form.get().matchers.get(i).map(|m| m.pattern.clone()).unwrap_or_default()
                                    on:input=move |e| {
                                        let v = event_target_value(&e);
                                        form.update(|f| { if let Some(m) = f.matchers.get_mut(i) { m.pattern = v; } });
                                        mark3();
                                    } /></td>
                                <td><input type="text"
                                    prop:value=move || form.get().matchers.get(i).map(|m| m.template.clone()).unwrap_or_default()
                                    on:input=move |e| {
                                        let v = event_target_value(&e);
                                        form.update(|f| { if let Some(m) = f.matchers.get_mut(i) { m.template = v; } });
                                        mark3();
                                    } /></td>
                                <td><input type="number" min="1" max="10"
                                    prop:value=move || form.get().matchers.get(i).map(|m| m.max_items.to_string()).unwrap_or_default()
                                    on:input=move |e| {
                                        let v = event_target_value(&e).parse().unwrap_or(5);
                                        form.update(|f| { if let Some(m) = f.matchers.get_mut(i) { m.max_items = v; } });
                                        mark3();
                                    } /></td>
                                <td class="center"><button class="danger" on:click=rm>"删除"</button></td>
                            </tr>
                        }
                    }).collect::<Vec<_>>()
                }}
            </tbody>
        </table>
        <div style="margin-top:6px">
            <button on:click=add_m>"添加 Matcher"</button>
        </div>
    }
}
