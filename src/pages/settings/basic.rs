//! 「基础配置」标签页（DESIGN §2 ①）：ID / 显示名 / 命令行（含粘贴解析 FR-2.12）/
//! 平台变体（FR-2.13）/ 工作目录 / shell / 捕获模式 / 交互终端 / 环境变量表（含行级平台列）。
//! 页内小节「平台变体」仅编辑模式显示（show_variants），新建向导不显示（DESIGN §5）。

use leptos::*;

use crate::bridge;
use crate::dto::{PastedLineDto, PlatformInfoDto};

use super::form::{set_form_value, FormState};

/// 平台变体字段写入：读出子表单 → mutate → 写回（成对出现在 input/change 处理器中）
fn set_platform_field(
    form: RwSignal<FormState>,
    platform: &str,
    mutate: impl FnOnce(&mut super::form::PlatformForm),
    mark0: impl Fn() + Copy,
) {
    form.update(|f| {
        let mut pf = f.platform_get(platform);
        mutate(&mut pf);
        f.platform_set(platform, pf);
    });
    mark0();
}
use super::panels::registry_platforms;

pub fn basic_panel(
    form: RwSignal<FormState>,
    dirty: RwSignal<Vec<bool>>,
    is_new: bool,
    show_variants: bool,
    platform_info: RwSignal<PlatformInfoDto>,
    on_preview: impl Fn(()) + Copy + 'static,
) -> impl IntoView {
    let mark0 = move || dirty.update(|d| d[0] = true);

    // 粘贴解析（FR-2.12）：自动（paste 事件）+ 「解析」按钮兜底
    let paste_hint = create_rw_signal(String::new());
    let parse_line = move |line: String| {
        let args = bridge::to_js(&line);
        spawn_local(async move {
            match bridge::invoke("parse_pasted_line", Some(args)).await {
                Ok(v) => match serde_wasm_bindgen::from_value::<PastedLineDto>(v) {
                    Ok(parsed) => {
                        let n = parsed.env.len();
                        set_form_value(form, move |f| {
                            f.command_line = parsed.command_line.clone();
                            for e in parsed.env {
                                // 重复 KEY 就地覆盖（与 shell 语义一致）
                                if let Some(slot) = f.env.iter_mut().find(|x| x.key == e.key) {
                                    slot.value = e.value.clone();
                                    slot.expand = e.expand;
                                    slot.platform = e.platform.clone();
                                } else {
                                    f.env.push(e);
                                }
                            }
                        });
                        paste_hint.set(if n > 0 {
                            format!("已识别 {n} 个前缀环境变量，已填入下方环境变量表")
                        } else {
                            String::new()
                        });
                    }
                    Err(e) => paste_hint.set(format!("解析结果读取失败：{e}")),
                },
                Err(e) => {
                    // 「缺少命令」等解析错误：原样保留输入，不做任何拆分
                    let msg = e.as_string().unwrap_or_else(|| format!("{e:?}"));
                    paste_hint.set(msg);
                }
            }
        });
    };
    let on_paste = move |e: web_sys::Event| {
        // leptos 0.6 将 paste 事件类型定义为 Event（ClipboardEvent 不稳定），
        // 经 Reflect 读取 event.clipboardData.getData("text/plain")
        use wasm_bindgen::JsCast;
        use wasm_bindgen::JsValue;
        let dt = js_sys::Reflect::get(e.as_ref(), &JsValue::from_str("clipboardData"))
            .unwrap_or(JsValue::UNDEFINED);
        if dt.is_undefined() || dt.is_null() {
            return;
        }
        let Ok(get) = js_sys::Reflect::get(&dt, &JsValue::from_str("getData"))
            .and_then(|f| f.dyn_into::<js_sys::Function>())
        else {
            return;
        };
        let text = get
            .call1(&dt, &JsValue::from_str("text/plain"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        // 仅当粘贴文本形如「前缀 KEY=VALUE …」时接管输入（FR-2.12：否则原样插入）。
        // 同步前置判断避免 prevent_default 与异步解析结果竞态
        let looks_env_prefixed = text
            .split_whitespace()
            .next()
            .and_then(|t| t.split_once('='))
            .map(|(k, _)| {
                !k.is_empty()
                    && k.chars()
                        .next()
                        .map(|c| c.is_ascii_alphabetic() || c == '_')
                        .unwrap_or(false)
                    && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
            .unwrap_or(false);
        if !looks_env_prefixed {
            return;
        }
        // 解析结果接管整个输入：避免默认插入与解析结果叠加
        e.prevent_default();
        mark0();
        parse_line(text);
    };
    let on_parse_click = move |_| {
        let line = form.get_untracked().command_line;
        if line.trim().is_empty() {
            paste_hint.set("命令行为空，无可解析内容".into());
            return;
        }
        paste_hint.set(String::new());
        parse_line(line);
    };

    // 平台变体子标签：0 = 默认，1.. = 注册表平台
    let psub = create_rw_signal(0usize);
    let chain_text = move || {
        let info = platform_info.get();
        if info.chain.is_empty() {
            String::new()
        } else {
            let native = if info.current.is_some() {
                format!("（本机 OS：{}）", info.native_os)
            } else {
                String::new()
            };
            let cur = info
                .current
                .clone()
                .map(|c| format!("当前平台：{c}"))
                .unwrap_or_else(|| format!("当前平台：{}（自动检测）", info.native_os));
            format!("{cur}{native}；解析链：{}", info.chain.join(" → "))
        }
    };

    view! {
        <div class="field-grid">
            <label>"ID（唯一）"</label>
            <input type="text" prop:value=move || form.get().id
                readonly=!is_new
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.id = v);
                    mark0();
                } />
            <label>"显示名"</label>
            <input type="text" prop:value=move || form.get().name.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.name = v);
                    mark0();
                } />
            <label>"命令行"</label>
            <div>
                <input type="text"
                    style="width:100%"
                    placeholder=r#"支持粘贴整行：ENV=… ENV2=… 程序 --参数（如 ENABLE_CORS=true app --port=1123）"#
                    prop:value=move || form.get().command_line.clone()
                    on:paste=on_paste
                    on:input=move |e| {
                        let v = event_target_value(&e);
                        set_form_value(form, move |f| f.command_line = v);
                        mark0();
                    } />
                <div style="display:flex;gap:8px;align-items:center;margin-top:2px;">
                    <button on:click=on_parse_click>"解析"</button>
                    <span class="hint">{move || paste_hint.get().clone()}</span>
                </div>
            </div>
            <span class="hint">"粘贴自动识别前缀 KEY=VALUE 为环境变量、其余为程序 + 参数；也可粘贴后点「解析」。默认不经 shell（FR-3.6）"</span>
            {move || {
                let eff = form.get().effective_command_line;
                if is_new || eff.is_empty() || eff == form.get().command_line {
                    view! { <span></span> }.into_view()
                } else {
                    view! { <span class="hint" style="color:var(--warn)">{format!("本机实际执行：{eff}")}</span> }.into_view()
                }
            }}
            <label>"经 shell 启动"</label>
            <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                <input type="checkbox" prop:checked=move || form.get().shell
                    on:change=move |e| {
                        let v = event_target_checked(&e);
                        set_form_value(form, move |f| f.shell = v);
                        mark0();
                    } />
                "经登录 shell（bash/zsh）启动，继承终端环境（PATH、nvm 等）；命令行按原样重组，不支持管道等 shell 语法"
            </label>
            <label>"工作目录"</label>
            <input type="text" placeholder="默认用户目录，支持 ~"
                prop:value=move || form.get().cwd.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.cwd = v);
                    mark0();
                } />
            <label>"启动捕获模式"</label>
            <select on:change=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.spawn_mode = v);
                    mark0();
                }
                prop:value=move || form.get().spawn_mode.clone()>
                <option value="Pipe">"管道（pipe）"</option>
                <option value="Pty">"伪终端（pty，启用终端视图）"</option>
            </select>
            <label>"交互终端"</label>
            <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                <input type="checkbox" prop:checked=move || form.get().term_interactive
                    on:change=move |e| {
                        let v = event_target_checked(&e);
                        set_form_value(form, move |f| f.term_interactive = v);
                        mark0();
                    } />
                "终端视图可写（键盘输入转发 stdin；需 pty 模式）"
            </label>
        </div>

        // 平台变体（FR-2.13 / DESIGN §4.7）：仅编辑模式
        {move || {
            if !show_variants {
                return view! { <div></div> }.into_view();
            }
            view! {
                <h3 style="margin:14px 0 6px;">"平台变体"</h3>
                <div class="hint" style="margin:0 0 6px;">{chain_text}</div>
                {platform_variant_section(form, mark0, psub, platform_info)}
            }.into_view()
        }}

        // 环境变量（原独立标签页并入基础配置，DESIGN §2 ①）
        <h3 style="margin:14px 0 6px;">"环境变量"</h3>
        {env_table(form, mark0, platform_info, on_preview)}
    }
}

/// 平台变体子区（DESIGN §4.7）：默认 / 各注册表平台子标签；
/// 平台子页字段留空 = 跟随默认，本机解析链上的平台带「本机」角标
fn platform_variant_section(
    form: RwSignal<FormState>,
    mark0: impl Fn() + Copy + 'static,
    psub: RwSignal<usize>,
    platform_info: RwSignal<PlatformInfoDto>,
) -> impl IntoView {
    let names = move || {
        let mut v = vec!["默认".to_string()];
        v.extend(registry_platforms(platform_info));
        v
    };
    let chain = move || platform_info.get().chain;

    view! {
        <div class="tabbar" style="margin-bottom:8px;">
            {move || {
                names().iter().enumerate().map(|(i, name)| {
                    let name = name.clone();
                    let native = i > 0 && chain().iter().any(|c| c == &name);
                    view! {
                        <button class="tab" class:active=move || psub.get() == i
                            on:click=move |_| psub.set(i)>
                            {move || {
                                if native { format!("{name}（本机）") } else { name.clone() }
                            }}
                        </button>
                    }
                }).collect::<Vec<_>>()
            }}
        </div>
        {move || {
            let i = psub.get();
            if i == 0 {
                view! { <span class="hint">"「默认」即左侧基础字段；本机未命中任何平台变体时使用默认值"</span> }.into_view()
            } else {
                let platform = names().get(i).cloned().unwrap_or_default();
                platform_variant_fields(form, platform, mark0, platform_info).into_view()
            }
        }}
    }
}

/// 单个平台的变体覆盖输入（DESIGN §4.7）
fn platform_variant_fields(
    form: RwSignal<FormState>,
    platform: String,
    mark0: impl Fn() + Copy + 'static,
    platform_info: RwSignal<PlatformInfoDto>,
) -> impl IntoView {
    // 每个闭包独立持有所需的克隆（视图宏会将闭包逐个 move）
    let family_hint = {
        let p = platform.clone();
        move || {
            platform_info
                .get()
                .platforms
                .into_iter()
                .find(|d| d.name == p)
                .and_then(|d| d.family)
                .map(|f| format!("未配置的字段回退到家族平台 {f} 的变体，再回退默认值"))
                .unwrap_or_else(|| "留空的字段使用默认值".to_string())
        }
    };
    let label = {
        let p = platform.clone();
        move || format!("命令行（{p}）")
    };
    let label_cwd = {
        let p = platform.clone();
        move || format!("工作目录（{p}）")
    };
    let label_sm = {
        let p = platform.clone();
        move || format!("捕获模式（{p}）")
    };
    let label_ti = {
        let p = platform.clone();
        move || format!("交互终端（{p}）")
    };
    let g_cmd = {
        let p = platform.clone();
        move || form.get().platform_get(&p).command_line
    };
    let g_cwd = {
        let p = platform.clone();
        move || form.get().platform_get(&p).cwd
    };
    let g_sm = {
        let p = platform.clone();
        move || form.get().platform_get(&p).spawn_mode
    };
    let g_ti = {
        let p = platform.clone();
        move || form.get().platform_get(&p).term_interactive
    };
    // 四个字段处理器各自持有克隆（move 闭包独占所有权）
    let p_cmd = platform.clone();
    let p_cwd = platform.clone();
    let p_sm = platform.clone();
    view! {
        <div class="hint" style="margin:0 0 6px;">{family_hint}</div>
        <div class="field-grid">
            <label>{label}</label>
            <input type="text" placeholder="留空 = 使用默认命令行"
                prop:value=g_cmd
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_platform_field(form, &p_cmd, |pf| pf.command_line = v, mark0);
                } />
            <label>{label_cwd}</label>
            <input type="text" placeholder="留空 = 使用默认工作目录，支持 ~"
                prop:value=g_cwd
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_platform_field(form, &p_cwd, |pf| pf.cwd = v, mark0);
                } />
            <label>{label_sm}</label>
            <select
                on:change=move |e| {
                    let v = event_target_value(&e);
                    set_platform_field(form, &p_sm, |pf| pf.spawn_mode = v, mark0);
                }
                prop:value=g_sm>
                <option value="">"跟随默认"</option>
                <option value="Pipe">"管道（pipe）"</option>
                <option value="Pty">"伪终端（pty）"</option>
            </select>
            <label>{label_ti}</label>
            <select
                on:change=move |e| {
                    let v = event_target_value(&e);
                    set_platform_field(form, &platform, |pf| pf.term_interactive = v, mark0);
                }
                prop:value=g_ti>
                <option value="">"跟随默认"</option>
                <option value="true">"true"</option>
                <option value="false">"false"</option>
            </select>
        </div>
    }
}

/// 环境变量表（FR-2.3 + FR-2.13 行级平台列）；原 env_panel 并入基础配置页
fn env_table(
    form: RwSignal<FormState>,
    mark0: impl Fn() + Copy + 'static,
    platform_info: RwSignal<PlatformInfoDto>,
    on_preview: impl Fn(()) + Copy + 'static,
) -> impl IntoView {
    let add_row = move |_| {
        form.update(|f| {
            f.env.push(crate::dto::EnvEntry {
                key: String::new(),
                value: String::new(),
                expand: false,
                platform: None,
            })
        });
        mark0();
    };
    let preview_btn = on_preview;
    view! {
        <div class="hint" style="margin:0 0 8px;">
            "按书写顺序注入；「平台」列选特定平台后该行仅在该平台生效，同名键平台行覆盖「全部平台」行（DESIGN §4.3）；勾选「替换」后值中的 $&#123;VAR&#125; 先查本表在前条目、再查系统环境（FR-2.3）"
        </div>
        <table class="kv">
            <thead>
                <tr><th>"Key"</th><th>"Value"</th><th>"替换"</th><th>"平台"</th><th></th></tr>
            </thead>
            <tbody>
                {move || {
                    form.get().env.iter().enumerate().map(|(i, _)| {
                        let rm = move |_| {
                            form.update(|f| { f.env.remove(i); });
                            mark0();
                        };
                        view! {
                            <tr>
                                <td><input type="text"
                                    prop:value=move || form.get().env.get(i).map(|e| e.key.clone()).unwrap_or_default()
                                    on:input=move |e| {
                                        let v = event_target_value(&e);
                                        form.update(|f| { if let Some(en) = f.env.get_mut(i) { en.key = v; } });
                                        mark0();
                                    } /></td>
                                <td><input type="text"
                                    prop:value=move || form.get().env.get(i).map(|e| e.value.clone()).unwrap_or_default()
                                    on:input=move |e| {
                                        let v = event_target_value(&e);
                                        form.update(|f| { if let Some(en) = f.env.get_mut(i) { en.value = v; } });
                                        mark0();
                                    } /></td>
                                <td class="center"><input type="checkbox"
                                    prop:checked=move || form.get().env.get(i).map(|e| e.expand).unwrap_or(false)
                                    on:change=move |e| {
                                        let v = event_target_checked(&e);
                                        form.update(|f| { if let Some(en) = f.env.get_mut(i) { en.expand = v; } });
                                        mark0();
                                    } /></td>
                                <td>
                                    {platform_select(form, i, mark0, platform_info)}
                                </td>
                                <td class="center"><button class="danger" on:click=rm>"删除"</button></td>
                            </tr>
                        }
                    }).collect::<Vec<_>>()
                }}
            </tbody>
        </table>
        <div style="margin-top:6px">
            <button on:click=add_row>"添加一行"</button>
            <button on:click=move |_| preview_btn(())>"预览展开结果"</button>
        </div>
    }
}

/// 单行平台下拉（全部平台 + 注册表平台，DESIGN §4.7）
fn platform_select(
    form: RwSignal<FormState>,
    i: usize,
    mark0: impl Fn() + Copy + 'static,
    platform_info: RwSignal<PlatformInfoDto>,
) -> impl IntoView {
    let names = move || registry_platforms(platform_info);
    view! {
        <select style="min-width:110px"
            on:change=move |e| {
                let v = event_target_value(&e);
                form.update(|f| {
                    if let Some(en) = f.env.get_mut(i) {
                        en.platform = (!v.is_empty()).then(|| v.clone());
                    }
                });
                mark0();
            }
            prop:value=move || form.get().env.get(i).and_then(|e| e.platform.clone()).unwrap_or_default()>
            <option value="">"全部平台"</option>
            {move || {
                names().into_iter().map(|n| {
                    let n2 = n.clone();
                    view! { <option value={n.clone()}>{move || n2.clone()}</option> }
                }).collect::<Vec<_>>()
            }}
        </select>
    }
}
