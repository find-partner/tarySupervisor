//! 表格式标签页：启动策略（DESIGN §2 ②）、日志（含 redirect_stderr）、快捷项（FR-5）。
//! 环境变量表已并入「基础配置」页（basic.rs 复用 env_table）。

use leptos::*;

use crate::dto::{MatcherConfig, PlatformInfoDto};

use super::form::{set_form_value, FormState};

/// 启动策略（DESIGN §2 ②）：supervisor 如何对待进程——全平台共用
pub fn strategy_panel(form: RwSignal<FormState>, dirty: RwSignal<Vec<bool>>) -> impl IntoView {
    let mark1 = move || dirty.update(|d| d[1] = true);
    view! {
        <div class="field-grid">
            <label>"autostart"</label>
            <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                <input type="checkbox" prop:checked=move || form.get().autostart
                    on:change=move |e| {
                        let v = event_target_checked(&e);
                        set_form_value(form, move |f| f.autostart = v);
                        mark1();
                    } />
                "应用启动时自动启动"
            </label>
            <label>"autorestart"</label>
            <select on:change=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.autorestart = v);
                    mark1();
                }
                prop:value=move || form.get().autorestart.clone()>
                <option value="No">"no（不自动重启）"</option>
                <option value="OnFailure">"on-failure（异常退出时重启）"</option>
                <option value="Always">"always（总是重启）"</option>
            </select>
            <label>"startsecs"</label>
            <input type="number" min="0" prop:value=move || form.get().startsecs.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.startsecs = v);
                    mark1();
                } />
            <span class="hint">"进程存活超过该秒数（或产生首行输出）判定为运行中（默认 3）"</span>
            <label>"startretries"</label>
            <input type="number" min="0" max="10" prop:value=move || form.get().startretries.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.startretries = v);
                    mark1();
                } />
            <span class="hint">"启动重试次数上限，耗尽进入 FATAL（默认 5）"</span>
            <label>"stopwaitsecs"</label>
            <input type="number" min="1" prop:value=move || form.get().stopwaitsecs.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.stopwaitsecs = v);
                    mark1();
                } />
            <span class="hint">"优雅停止等待秒数，超时强杀整个进程树（默认 10）"</span>
        </div>
    }
}

/// 日志页（DESIGN §2 ③）：redirect_stderr（决定日志里有几路流）+ 文件落盘策略
pub fn log_panel(form: RwSignal<FormState>, dirty: RwSignal<Vec<bool>>) -> impl IntoView {
    let mark2 = move || dirty.update(|d| d[2] = true);
    view! {
        <div class="field-grid">
            <label>"redirect_stderr"</label>
            <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                <input type="checkbox" prop:checked=move || form.get().redirect_stderr
                    on:change=move |e| {
                        let v = event_target_checked(&e);
                        set_form_value(form, move |f| f.redirect_stderr = v);
                        mark2();
                    } />
                "stderr 并入 stdout 单流"
            </label>
            <label>"写入文件"</label>
            <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                <input type="checkbox" prop:checked=move || form.get().log_enabled
                    on:change=move |e| {
                        let v = event_target_checked(&e);
                        set_form_value(form, move |f| {
                            f.log_enabled = v;
                            // 勾选启用且路径为空 → 直接生成内置默认路径（清空后保存则由后端兜底生成）
                            if v && f.log_path.trim().is_empty() && !f.id.trim().is_empty() {
                                f.log_path = format!("~/.traysupervisor/logs/{}.log", f.id.trim());
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
        form.update(|f| {
            f.matchers.push(MatcherConfig {
                pattern: String::new(),
                template: "🌐 打开 {0}".into(),
                max_items: 5,
            })
        });
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

/// 平台注册表平台名列表（信号驱动；注册表未加载时回退内置三项）
pub fn registry_platforms(info: RwSignal<PlatformInfoDto>) -> Vec<String> {
    let v = info.get();
    if v.platforms.is_empty() {
        vec!["windows".into(), "macos".into(), "linux".into()]
    } else {
        v.platforms.into_iter().map(|p| p.name).collect()
    }
}
