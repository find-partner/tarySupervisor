//! 设置窗口（FR-2 / SRS §5.2）：左侧服务列表 + 右侧编辑表单；
//! 环境变量 KV 表格（Key/Value/替换开关/删除 + 添加一行）；Matcher 表格；
//! 校验错误逐项展示；env 展开预览。

use leptos::*;

use crate::bridge;
use crate::dto::{ConfigInvalidEv, EnvEntry, MatcherConfig, ServiceBriefOut, ServiceInput};

#[derive(Clone, Debug, Default)]
struct FormState {
    id: String,
    name: String,
    command_line: String,
    cwd: String,
    env: Vec<EnvEntry>,
    redirect_stderr: bool,
    autostart: bool,
    autorestart: String,
    startsecs: String,
    startretries: String,
    stopwaitsecs: String,
    spawn_mode: String,
    log_enabled: bool,
    log_path: String,
    log_max_bytes: String,
    log_max_files: String,
    log_compress: bool,
    matchers: Vec<MatcherConfig>,
    /// 新建（id 可编辑）还是编辑已有服务
    is_new: bool,
}

fn form_from_brief(b: &ServiceBriefOut) -> FormState {
    FormState {
        id: b.config.id.clone(),
        name: b.config.name.clone(),
        command_line: b.command_line.clone(),
        cwd: b.config.cwd.clone().unwrap_or_default(),
        env: b.config.env.clone(),
        redirect_stderr: b.config.redirect_stderr,
        autostart: b.config.autostart,
        autorestart: b.config.autorestart.clone(),
        startsecs: b.config.startsecs.to_string(),
        startretries: b.config.startretries.to_string(),
        stopwaitsecs: b.config.stop_timeout_secs.to_string(),
        spawn_mode: b.config.spawn_mode.clone(),
        log_enabled: b.config.log.file.is_some(),
        log_path: b
            .config
            .log
            .file
            .as_ref()
            .map(|f| f.path.clone())
            .unwrap_or_default(),
        log_max_bytes: b
            .config
            .log
            .file
            .as_ref()
            .map(|f| f.max_bytes.to_string())
            .unwrap_or_else(|| "10485760".into()),
        log_max_files: b
            .config
            .log
            .file
            .as_ref()
            .map(|f| f.max_files.to_string())
            .unwrap_or_else(|| "3".into()),
        log_compress: b.config.log.file.as_ref().map(|f| f.compress).unwrap_or(false),
        matchers: b.config.matchers.clone(),
        is_new: false,
    }
}

fn input_from_form(f: &FormState) -> ServiceInput {
    ServiceInput {
        id: f.id.trim().to_string(),
        name: f.name.trim().to_string(),
        command_line: f.command_line.clone(),
        cwd: {
            let c = f.cwd.trim().to_string();
            (!c.is_empty()).then_some(c)
        },
        env: f.env.clone(),
        redirect_stderr: f.redirect_stderr,
        autostart: f.autostart,
        autorestart: f.autorestart.clone(),
        startsecs: f.startsecs.trim().parse().unwrap_or(3),
        startretries: f.startretries.trim().parse().unwrap_or(5),
        stopwaitsecs: f.stopwaitsecs.trim().parse().unwrap_or(10),
        spawn_mode: f.spawn_mode.clone(),
        log_file_enabled: f.log_enabled && !f.log_path.trim().is_empty(),
        log_path: (!f.log_path.trim().is_empty()).then(|| f.log_path.trim().to_string()),
        log_max_bytes: f.log_max_bytes.trim().parse().unwrap_or(10_485_760),
        log_max_files: f.log_max_files.trim().parse().unwrap_or(3),
        log_compress: f.log_compress,
        matchers: f.matchers.clone(),
    }
}

fn blank_form() -> FormState {
    FormState {
        id: String::new(),
        name: String::new(),
        command_line: String::new(),
        cwd: String::new(),
        env: vec![],
        redirect_stderr: false,
        autostart: false,
        autorestart: "No".into(),
        startsecs: "3".into(),
        startretries: "5".into(),
        stopwaitsecs: "10".into(),
        spawn_mode: "Pipe".into(),
        log_enabled: false,
        log_path: String::new(),
        log_max_bytes: "10485760".into(),
        log_max_files: "3".into(),
        log_compress: false,
        matchers: vec![],
        is_new: true,
    }
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let (list, set_list) = create_signal::<Vec<ServiceBriefOut>>(Vec::new());
    let (form, set_form) = create_signal(blank_form());
    let (errors, set_errors) = create_signal::<Vec<String>>(Vec::new());
    let (banner, set_banner) = create_signal::<Vec<String>>(Vec::new());
    let (preview, set_preview) = create_signal::<Option<Result<String, String>>>(None);
    let (selected_id, set_selected) = create_signal::<Option<String>>(None);

    let refresh = move || {
        spawn_local(async move {
            let l: Vec<ServiceBriefOut> = bridge::invoke_t("list_services", None).await;
            set_list.set(l);
        });
    };
    refresh();

    // 配置热更/校验失败横幅（FR-2.6）
    spawn_local(async move {
        let _ = bridge::listen("config-changed", move |_| {
            set_banner.set(Vec::new());
            refresh();
        })
        .await;
        let _ = bridge
            ::listen("config-invalid", move |payload| {
                if let Ok(ev) = serde_wasm_bindgen::from_value::<ConfigInvalidEv>(payload) {
                    set_banner.set(ev.errors);
                    refresh();
                }
            })
            .await;
    });

    // ---- 表单动作 ----
    let select = move |b: ServiceBriefOut| {
        set_selected.set(Some(b.config.id.clone()));
        set_form.set(form_from_brief(&b));
        set_errors.set(Vec::new());
        set_preview.set(None);
    };
    let new_service = move |_| {
        set_selected.set(None);
        set_form.set(blank_form());
        set_errors.set(Vec::new());
        set_preview.set(None);
    };
    let save = move |_| {
        let f = form.get();
        spawn_local(async move {
            let input = input_from_form(&f);
            let args = bridge::invoke_args_input(&input);
            match bridge::invoke_errors("upsert_service", Some(args)).await {
                Ok(()) => {
                    set_errors.set(Vec::new());
                    set_preview.set(None);
                    set_selected.set(Some(input.id));
                    refresh();
                }
                Err(errs) => set_errors.set(errs),
            }
        });
    };
    let del = move |_| {
        let Some(id) = selected_id.get() else { return };
        let running = list
            .get()
            .iter()
            .find(|b| b.config.id == id)
            .map(|b| b.state == "RUNNING" || b.state == "STARTING")
            .unwrap_or(false);
        let msg = if running {
            format!("服务 {id} 正在运行，删除前将先停止。确认删除？")
        } else {
            format!("确认删除服务 {id}？")
        };
        if !web_sys::window().map(|w| w.confirm_with_message(&msg).unwrap_or(false)).unwrap_or(false) {
            return;
        }
        spawn_local(async move {
            match bridge::invoke_errors("delete_service", Some(bridge::invoke_args_id(&id))).await {
                Ok(()) => {
                    set_selected.set(None);
                    set_form.set(blank_form());
                    set_errors.set(Vec::new());
                    refresh();
                }
                Err(errs) => set_errors.set(errs),
            }
        });
    };
    let do_preview = move |_| {
        let f = form.get();
        spawn_local(async move {
            let args = bridge::invoke_args_input(&input_from_form(&f));
            let raw = bridge::invoke("get_env_resolved_preview", Some(args)).await;
            match raw {
                Ok(v) => {
                    let pairs: Vec<(String, String)> =
                        serde_wasm_bindgen::from_value(v).unwrap_or_default();
                    set_preview.set(Some(Ok(
                        pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\n")
                    )));
                }
                Err(e) => {
                    let msg = e
                        .as_string()
                        .unwrap_or_else(|| format!("{e:?}"));
                    set_preview.set(Some(Err(msg)));
                }
            }
        });
    };

    view! {
        <div class="settings">
            <div class="svc-list">
                <h3>"服务（" {move || list.get().len()} "）"</h3>
                {move || {
                    let data = list.get();
                    data.iter().map(|b| {
                        let b = b.clone();
                        let st = b.state.clone();
                        let cls = b.state_class();
                        let id = b.config.id.clone();
                        let name = b.display_name().to_string();
                        view! {
                            <div class="svc-item"
                                class:active=move || selected_id.get().as_deref() == Some(id.as_str())
                                on:click=move |_| select(b.clone())>
                                <span class="dot" class={cls}></span>
                                <span class="name">{name}</span>
                                <span class="st">{st}</span>
                            </div>
                        }
                    }).collect::<Vec<_>>()
                }}
                <div style="margin-top:8px">
                    <button on:click=new_service>"+ 新建服务"</button>
                </div>
            </div>

            <div class="editor">
                <h2>
                    {move || if form.get().is_new { "新建服务".to_string() }
                        else { format!("编辑：{}", if form.get().name.is_empty() { form.get().id } else { form.get().name.clone() }) }}
                </h2>

                {move || {
                    let errs = banner.get();
                    if errs.is_empty() { view! { <div></div> }.into_view() }
                    else {
                        view! {
                            <div class="errors">
                                <b>"配置文件热加载校验失败（沿用上一次有效配置）："</b>
                                <ul>{errs.into_iter().map(|e| view! { <li>{e}</li> }).collect::<Vec<_>>()}</ul>
                            </div>
                        }.into_view()
                    }
                }}

                <div class="field-grid">
                    <label>"ID（唯一）"</label>
                    <input type="text" prop:value=move || form.get().id
                        readonly=move || !form.get().is_new
                        on:input=move |e| set_form.update(|f| f.id = event_target_value(&e)) />
                    <label>"显示名"</label>
                    <input type="text" prop:value=move || form.get().name.clone()
                        on:input=move |e| set_form.update(|f| f.name = event_target_value(&e)) />
                    <label>"命令行"</label>
                    <input type="text" placeholder=r#"python -m http.server 8000 --directory "C:\work\docs""#
                        prop:value=move || form.get().command_line.clone()
                        on:input=move |e| set_form.update(|f| f.command_line = event_target_value(&e)) />
                    <span class="hint">"程序 + 参数，空格分隔，双引号包裹含空格部分；不经 shell（FR-3.6）"</span>
                    <label>"工作目录"</label>
                    <input type="text" placeholder="默认用户目录，支持 ~"
                        prop:value=move || form.get().cwd.clone()
                        on:input=move |e| set_form.update(|f| f.cwd = event_target_value(&e)) />
                    <label>"启动捕获模式"</label>
                    <select on:change=move |e| set_form.update(|f| f.spawn_mode = event_target_value(&e))
                        prop:value=move || form.get().spawn_mode.clone()>
                        <option value="Pipe">"管道（pipe）"</option>
                        <option value="Pty">"伪终端（pty，启用只读终端视图）"</option>
                    </select>
                    <label>"autostart"</label>
                    <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                        <input type="checkbox" prop:checked=move || form.get().autostart
                            on:change=move |e| set_form.update(|f| f.autostart = event_target_checked(&e)) />
                        "应用启动时自动启动"
                    </label>
                    <label>"autorestart"</label>
                    <select on:change=move |e| set_form.update(|f| f.autorestart = event_target_value(&e))
                        prop:value=move || form.get().autorestart.clone()>
                        <option value="No">"no（不自动重启）"</option>
                        <option value="OnFailure">"on-failure（异常退出时重启）"</option>
                        <option value="Always">"always（总是重启）"</option>
                    </select>
                    <label>"startsecs"</label>
                    <input type="number" min="0" prop:value=move || form.get().startsecs.clone()
                        on:input=move |e| set_form.update(|f| f.startsecs = event_target_value(&e)) />
                    <label>"startretries"</label>
                    <input type="number" min="0" max="10" prop:value=move || form.get().startretries.clone()
                        on:input=move |e| set_form.update(|f| f.startretries = event_target_value(&e)) />
                    <label>"stopwaitsecs"</label>
                    <input type="number" min="1" prop:value=move || form.get().stopwaitsecs.clone()
                        on:input=move |e| set_form.update(|f| f.stopwaitsecs = event_target_value(&e)) />
                    <label>"redirect_stderr"</label>
                    <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                        <input type="checkbox" prop:checked=move || form.get().redirect_stderr
                            on:change=move |e| set_form.update(|f| f.redirect_stderr = event_target_checked(&e)) />
                        "stderr 并入 stdout 单流"
                    </label>
                </div>

                <div class="section-title">"日志文件（可选；不启用 = 仅内存环形缓冲）"</div>
                <div class="field-grid">
                    <label>"写入文件"</label>
                    <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                        <input type="checkbox" prop:checked=move || form.get().log_enabled
                            on:change=move |e| set_form.update(|f| f.log_enabled = event_target_checked(&e)) />
                        "启用文件日志"
                    </label>
                    <label>"文件路径"</label>
                    <input type="text" placeholder="~/.traysupervisor/logs/xxx.log"
                        prop:value=move || form.get().log_path.clone()
                        on:input=move |e| set_form.update(|f| f.log_path = event_target_value(&e)) />
                    <label>"单文件上限（字节）"</label>
                    <input type="number" min="0" prop:value=move || form.get().log_max_bytes.clone()
                        on:input=move |e| set_form.update(|f| f.log_max_bytes = event_target_value(&e)) />
                    <span class="hint">"0 = 不轮转；超出后按 .1 .2 … 分块"</span>
                    <label>"保留份数"</label>
                    <input type="number" min="0" max="100" prop:value=move || form.get().log_max_files.clone()
                        on:input=move |e| set_form.update(|f| f.log_max_files = event_target_value(&e)) />
                    <label>"gzip 压缩旧分块"</label>
                    <input type="checkbox" prop:checked=move || form.get().log_compress
                        on:change=move |e| set_form.update(|f| f.log_compress = event_target_checked(&e)) />
                </div>

                <div class="section-title">"环境变量（按书写顺序注入；!前缀 = 开启 ${VAR} 替换）"</div>
                <table class="kv">
                    <thead>
                        <tr><th>"Key"</th><th>"Value"</th><th>"替换"</th><th></th></tr>
                    </thead>
                    <tbody>
                        {move || {
                            form.get().env.iter().enumerate().map(|(i, _)| {
                                let rm = move |_| {
                                    set_form.update(|f| { f.env.remove(i); });
                                };
                                view! {
                                    <tr>
                                        <td><input type="text"
                                            prop:value=move || form.get().env.get(i).map(|e| e.key.clone()).unwrap_or_default()
                                            on:input=move |e| set_form.update(|f| {
                                                if let Some(en) = f.env.get_mut(i) { en.key = event_target_value(&e); }
                                            }) /></td>
                                        <td><input type="text"
                                            prop:value=move || form.get().env.get(i).map(|e| e.value.clone()).unwrap_or_default()
                                            on:input=move |e| set_form.update(|f| {
                                                if let Some(en) = f.env.get_mut(i) { en.value = event_target_value(&e); }
                                            }) /></td>
                                        <td class="center"><input type="checkbox"
                                            prop:checked=move || form.get().env.get(i).map(|e| e.expand).unwrap_or(false)
                                            on:change=move |e| set_form.update(|f| {
                                                if let Some(en) = f.env.get_mut(i) { en.expand = event_target_checked(&e); }
                                            }) /></td>
                                        <td class="center"><button class="danger" on:click=rm>"删除"</button></td>
                                    </tr>
                                }
                            }).collect::<Vec<_>>()
                        }}
                    </tbody>
                </table>
                <div style="margin-top:6px">
                    <button on:click=move |_| set_form.update(|f| f.env.push(EnvEntry {
                        key: String::new(), value: String::new(), expand: false,
                    }))>"添加一行"</button>
                    <button on:click=do_preview>"预览展开结果"</button>
                </div>

                <div class="section-title">"Matcher（日志正则 → 托盘快捷项；留空启用内置 HTTP URL 规则）"</div>
                <table class="kv">
                    <thead>
                        <tr><th>"Pattern（Rust regex）"</th><th>"模板（{0} {1} 引用捕获组）"</th><th>"上限"</th><th></th></tr>
                    </thead>
                    <tbody>
                        {move || {
                            form.get().matchers.iter().enumerate().map(|(i, _)| {
                                let rm = move |_| {
                                    set_form.update(|f| { f.matchers.remove(i); });
                                };
                                view! {
                                    <tr>
                                        <td><input type="text"
                                            prop:value=move || form.get().matchers.get(i).map(|m| m.pattern.clone()).unwrap_or_default()
                                            on:input=move |e| set_form.update(|f| {
                                                if let Some(m) = f.matchers.get_mut(i) { m.pattern = event_target_value(&e); }
                                            }) /></td>
                                        <td><input type="text"
                                            prop:value=move || form.get().matchers.get(i).map(|m| m.template.clone()).unwrap_or_default()
                                            on:input=move |e| set_form.update(|f| {
                                                if let Some(m) = f.matchers.get_mut(i) { m.template = event_target_value(&e); }
                                            }) /></td>
                                        <td><input type="number" min="1" max="10"
                                            prop:value=move || form.get().matchers.get(i).map(|m| m.max_items.to_string()).unwrap_or_default()
                                            on:input=move |e| set_form.update(|f| {
                                                if let Some(m) = f.matchers.get_mut(i) { m.max_items = event_target_value(&e).parse().unwrap_or(5); }
                                            }) /></td>
                                        <td class="center"><button class="danger" on:click=rm>"删除"</button></td>
                                    </tr>
                                }
                            }).collect::<Vec<_>>()
                        }}
                    </tbody>
                </table>
                <div style="margin-top:6px">
                    <button on:click=move |_| set_form.update(|f| f.matchers.push(MatcherConfig {
                        pattern: String::new(), template: "🌐 打开 {0}".into(), max_items: 5,
                    }))>"添加 Matcher"</button>
                </div>

                {move || {
                    let errs = errors.get();
                    if errs.is_empty() { view! { <div></div> }.into_view() }
                    else {
                        view! {
                            <div class="errors">
                                <b>"校验失败（未保存）："</b>
                                <ul>{errs.into_iter().map(|e| view! { <li>{e}</li> }).collect::<Vec<_>>()}</ul>
                            </div>
                        }.into_view()
                    }
                }}

                {move || {
                    preview.get().map(|r| match r {
                        Ok(text) => view! { <div class="preview">{text}</div> }.into_view(),
                        Err(msg) => view! {
                            <div class="errors"><b>"env 展开失败："</b>{msg}</div>
                        }.into_view(),
                    })
                }}

                <div class="row-btns">
                    <button class="primary" on:click=save>"保存"</button>
                    <button class="danger" on:click=del>"删除此服务"</button>
                </div>
            </div>
        </div>
    }
}
