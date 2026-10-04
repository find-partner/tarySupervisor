//! 设置窗口（FR-2 / SRS FR-2A）：左侧服务列表 + 右侧三态编辑区——
//! 空态引导（FR-2.7）/ 编辑模式（四标签页、每页独立保存，FR-2.8）/
//! 新建向导（FR-2.9）；保存后对运行中服务提示"需重启生效"（FR-2.10）。

use leptos::*;

use crate::bridge;
use crate::dto::{ConfigInvalidEv, EnvEntry, MatcherConfig, ServiceBriefOut, ServiceInput};

const TAB_NAMES: [&str; 4] = ["基本设置", "环境变量", "日志", "快捷项"];
const WIZARD_STEPS: [&str; 4] = ["基本设置", "环境变量（可跳过）", "日志（可跳过）", "快捷项（可跳过）"];

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
    term_interactive: bool,
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
        term_interactive: b.config.term_interactive,
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
        term_interactive: f.term_interactive,
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
        term_interactive: false,
        log_enabled: false,
        log_path: String::new(),
        log_max_bytes: "10485760".into(),
        log_max_files: "3".into(),
        log_compress: false,
        matchers: vec![],
        is_new: true,
    }
}

/// 编辑模式的"每页独立保存"（FR-2.8）：以加载时快照 `loaded` 为基底，
/// 仅叠入 `tab` 页对应字段，避免把其他页的未保存变更一并提交。
/// 向导模式（新建）不走此合并——最后一步整体保存。
fn merged_for_save(loaded: &FormState, current: &FormState, tab: usize) -> FormState {
    let mut m = loaded.clone();
    match tab {
        0 => {
            m.id = current.id.clone();
            m.name = current.name.clone();
            m.command_line = current.command_line.clone();
            m.cwd = current.cwd.clone();
            m.spawn_mode = current.spawn_mode.clone();
            m.term_interactive = current.term_interactive;
            m.autostart = current.autostart;
            m.autorestart = current.autorestart.clone();
            m.startsecs = current.startsecs.clone();
            m.startretries = current.startretries.clone();
            m.stopwaitsecs = current.stopwaitsecs.clone();
            m.redirect_stderr = current.redirect_stderr;
        }
        1 => m.env = current.env.clone(),
        2 => {
            m.log_enabled = current.log_enabled;
            m.log_path = current.log_path.clone();
            m.log_max_bytes = current.log_max_bytes.clone();
            m.log_max_files = current.log_max_files.clone();
            m.log_compress = current.log_compress;
        }
        _ => m.matchers = current.matchers.clone(),
    }
    m
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// 空态引导（FR-2.7）
    Empty,
    /// 编辑已有服务（四标签页）
    Edit,
    /// 新建向导（FR-2.9），payload = 当前步 0..4
    Wizard(usize),
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let list = create_rw_signal::<Vec<ServiceBriefOut>>(Vec::new());
    // 需要传入子视图函数的信号一律用 RwSignal（leptos 0.6 的 create_signal 返回 ReadSignal/WriteSignal）
    let mode = create_rw_signal(Mode::Empty);
    let form = create_rw_signal(blank_form());
    // 编辑模式下的加载快照（每页独立保存的合并基底）
    let loaded = create_rw_signal(blank_form());
    let tab = create_rw_signal(0usize);
    let dirty = create_rw_signal(vec![false; 4]);
    let errors = create_rw_signal::<Vec<String>>(Vec::new());
    let step_err = create_rw_signal(String::new());
    let banner = create_rw_signal::<Vec<String>>(Vec::new());
    let preview = create_rw_signal::<Option<Result<String, String>>>(None);
    let selected_id = create_rw_signal::<Option<String>>(None);
    // Some((id, 显示名))：保存时服务正在运行 → 提示需重启生效（FR-2.10）
    let restart_hint = create_rw_signal::<Option<(String, String)>>(None);

    let refresh = move || {
        spawn_local(async move {
            let l: Vec<ServiceBriefOut> = bridge::invoke_t("list_services", None).await;
            list.set(l);
        });
    };
    refresh();

    // 配置热更/校验失败横幅（FR-2.6）
    spawn_local(async move {
        let _ = bridge
            ::listen("config-changed", move |_| {
                banner.set(Vec::new());
                refresh();
            })
            .await;
        let _ = bridge
            ::listen("config-invalid", move |payload| {
                if let Ok(ev) = serde_wasm_bindgen::from_value::<ConfigInvalidEv>(payload) {
                    banner.set(ev.errors);
                    refresh();
                }
            })
            .await;
    });

    let start_edit = move |b: &ServiceBriefOut| {
        selected_id.set(Some(b.config.id.clone()));
        form.set(form_from_brief(b));
        loaded.set(form_from_brief(b));
        mode.set(Mode::Edit);
        tab.set(0);
        errors.set(Vec::new());
        step_err.set(String::new());
        preview.set(None);
        dirty.set(vec![false; 4]);
        restart_hint.set(None);
    };
    let select = move |b: ServiceBriefOut| start_edit(&b);
    let start_wizard = move |_| {
        selected_id.set(None);
        form.set(blank_form());
        loaded.set(blank_form());
        mode.set(Mode::Wizard(0));
        tab.set(0);
        errors.set(Vec::new());
        step_err.set(String::new());
        preview.set(None);
        dirty.set(vec![false; 4]);
        restart_hint.set(None);
    };

    // ---- 保存（FR-2.8 每页独立 / FR-2.10 重启提示）----
    let save_from = move |src: FormState, from_wizard: bool| {
        spawn_local(async move {
            let input = input_from_form(&src);
            let was_running = list
                .get_untracked()
                .iter()
                .find(|b| b.config.id == input.id)
                .map(|b| b.state == "RUNNING" || b.state == "STARTING")
                .unwrap_or(false);
            let args = bridge::invoke_args_input(&input);
            match bridge::invoke_errors("upsert_service", Some(args)).await {
                Ok(()) => {
                    errors.set(Vec::new());
                    preview.set(None);
                    selected_id.set(Some(input.id.clone()));
                    dirty.set(vec![false; 4]);
                    restart_hint.set(None);
                    if from_wizard {
                        mode.set(Mode::Edit);
                    }
                    // 保存后以服务端规范化结果同步快照（name 默认、~ 展开等）
                    let saved_id = input.id.clone();
                    let fresh: Vec<ServiceBriefOut> =
                        bridge::invoke_t("list_services", None).await;
                    if let Some(b) = fresh.iter().find(|b| b.config.id == saved_id) {
                        loaded.set(form_from_brief(b));
                    }
                    list.set(fresh);
                    // 运行中服务：提示需重启生效（FR-2.10）
                    if was_running {
                        let name = loaded.get_untracked().name.clone();
                        restart_hint.set(Some((saved_id, name)));
                    }
                }
                Err(errs) => errors.set(errs),
            }
        });
    };
    let save_tab = move |tab_idx: usize| {
        let merged = merged_for_save(&loaded.get(), &form.get(), tab_idx);
        save_from(merged, false);
    };
    let save_wizard = move |_| save_from(form.get(), true);

    let restart_now = move |_| {
        if let Some((id, _)) = restart_hint.get_untracked() {
            spawn_local(async move {
                let _ = bridge::invoke("restart_service", Some(bridge::invoke_args_id(&id))).await;
                restart_hint.set(None);
                refresh();
            });
        }
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
                    selected_id.set(None);
                    form.set(blank_form());
                    loaded.set(blank_form());
                    mode.set(Mode::Empty);
                    errors.set(Vec::new());
                    dirty.set(vec![false; 4]);
                    refresh();
                }
                Err(errs) => errors.set(errs),
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
                    preview.set(Some(Ok(
                        pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\n")
                    )));
                }
                Err(e) => {
                    let msg = e
                        .as_string()
                        .unwrap_or_else(|| format!("{e:?}"));
                    preview.set(Some(Err(msg)));
                }
            }
        });
    };

    // ---- 向导步进（FR-2.9）：第①步要求 ID 与命令行非空 ----
    let wizard_next = move |_| {
        let m = mode.get();
        let Mode::Wizard(step) = m else { return };
        if step == 0 {
            let f = form.get();
            let mut err = Vec::new();
            if f.id.trim().is_empty() {
                err.push("ID 不能为空");
            }
            if f.command_line.trim().is_empty() {
                err.push("命令行不能为空");
            }
            if !err.is_empty() {
                step_err.set(err.join("；"));
                return;
            }
        }
        step_err.set(String::new());
        mode.set(Mode::Wizard((step + 1).min(3)));
    };
    let wizard_prev = move |_| {
        let Mode::Wizard(step) = mode.get() else { return };
        step_err.set(String::new());
        mode.set(Mode::Wizard(step.saturating_sub(1)));
    };

    // 编辑区三态视图在 view! 之外构造（宏对信号标识符的转换会改变类型）
    let editor_node = move || match mode.get() {
        Mode::Empty => empty_state().into_view(),
        Mode::Edit => edit_mode(
            form, tab, dirty, errors, banner, preview, restart_hint,
            selected_id, save_tab, restart_now, del, do_preview,
        ).into_view(),
        Mode::Wizard(step) => wizard_mode(
            step, form, dirty, errors, step_err,
            wizard_next, wizard_prev, save_wizard, do_preview,
        ).into_view(),
    };

    view! {
        <div class="settings">
            <div class="svc-list">
                <h3>"服务（" {move || list.get().len()} "）"</h3>
                {move || {
                    let data = list.get();
                    if data.is_empty() {
                        view! { <div class="hint" style="padding:4px 8px">"暂无服务——点击下方按钮新建"</div> }.into_view()
                    } else {
                        view! {
                            {data.iter().map(|b| {
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
                            }).collect::<Vec<_>>()}
                        }.into_view()
                    }
                }}
                <div style="margin-top:8px">
                    <button on:click=start_wizard>"+ 新建服务"</button>
                </div>
            </div>

            <div class="editor">
                {editor_node}
            </div>
        </div>
    }
}

// ---------- 空态（FR-2.7） ----------

fn empty_state() -> impl IntoView {
    view! {
        <div style="max-width:640px;margin:48px auto 0;padding:0 24px;text-align:left;">
            <h2 style="margin:0 0 10px;font-size:18px;">"欢迎使用 TraySupervisor"</h2>
            <p style="color:var(--muted);line-height:1.8;margin:0 0 18px;">
                "把你的命令行服务注册进来，由本工具作为子进程托管：从系统托盘启动 / 停止 / 重启，"
                "实时查看日志（可选按大小轮转的文件日志），在只读或交互终端中观察输出，"
                "并可用正则从日志中提取快捷链接（如 🌐 打开 http://localhost:3000）。"
            </p>
            <div class="section-title">"开始使用"</div>
            <ol style="line-height:2.1;color:var(--fg);margin:0 0 18px;padding-left:22px;">
                <li>"点击下方或左下角的 <b>「+ 新建服务」</b>，按向导填写命令行即可"</li>
                <li>"保存后从系统托盘菜单启动服务"</li>
                <li>"托盘菜单中查看状态、日志、终端视图与 Matcher 快捷链接"</li>
            </ol>
            <p class="hint" style="margin:0 0 16px;">"当前还没有任何服务配置——请新建配置，或编辑 ~/.traysupervisor/config.ini（支持热加载）。"</p>
        </div>
    }
}

// ---------- 编辑模式（FR-2.8 + FR-2.10） ----------

fn edit_mode(
    form: RwSignal<FormState>,
    tab: RwSignal<usize>,
    dirty: RwSignal<Vec<bool>>,
    errors: RwSignal<Vec<String>>,
    banner: RwSignal<Vec<String>>,
    preview: RwSignal<Option<Result<String, String>>>,
    restart_hint: RwSignal<Option<(String, String)>>,
    selected_id: RwSignal<Option<String>>,
    on_save_tab: impl Fn(usize) + Copy + 'static,
    on_restart: impl Fn(()) + Copy + 'static,
    on_delete: impl Fn(()) + Copy + 'static,
    on_preview: impl Fn(()) + Copy + 'static,
) -> impl IntoView {
    let title = move || {
        let f = form.get();
        format!("编辑：{}", if f.name.is_empty() { f.id } else { f.name })
    };
    let save_this = move |_| on_save_tab(tab.get_untracked());

    view! {
        <h2>{title}</h2>

        {move || {
            let Some((id, name)) = restart_hint.get() else {
                return view! { <div></div> }.into_view();
            };
            let who = if name.is_empty() { id.clone() } else { name };
            let on_r = on_restart;
            view! {
                <div class="errors" style="background:#fff8e6;border-color:#f0d08a;color:var(--warn);">
                    <b>{format!("配置已保存——{who} 正在运行，需重启后新配置才会生效。")}</b>
                    <div style="margin-top:6px;display:flex;gap:8px;">
                        <button class="primary" on:click=move |_| on_r(())>"立即重启"</button>
                    </div>
                </div>
            }.into_view()
        }}

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

        <div class="tabbar">
            {TAB_NAMES.iter().enumerate().map(|(i, name)| {
                let i = i;
                view! {
                    <button class="tab"
                        class:active=move || tab.get() == i
                        on:click=move |_| tab.set(i)>
                        {move || name.to_string()}
                        {move || if dirty.get().get(i).copied().unwrap_or(false) { " •" } else { "" }}
                    </button>
                }
            }).collect::<Vec<_>>()}
        </div>

        {move || match tab.get() {
            0 => basic_panel(form, dirty, false).into_view(),
            1 => env_panel(form, dirty, on_preview).into_view(),
            2 => log_panel(form, dirty).into_view(),
            _ => matcher_panel(form, dirty).into_view(),
        }}

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
            <button class="primary" on:click=save_this>{move || format!("保存「{}」", TAB_NAMES[tab.get()])}</button>
            {move || {
                let _ = selected_id.get();
                view! { <button class="danger" on:click=move |_| on_delete(())>"删除此服务"</button> }
            }}
        </div>
    }
}

// ---------- 新建向导（FR-2.9） ----------

fn wizard_mode(
    step: usize,
    form: RwSignal<FormState>,
    dirty: RwSignal<Vec<bool>>,
    errors: RwSignal<Vec<String>>,
    step_err: RwSignal<String>,
    on_next: impl Fn(()) + Copy + 'static,
    on_prev: impl Fn(()) + Copy + 'static,
    on_save: impl Fn(()) + Copy + 'static,
    on_preview: impl Fn(()) + Copy + 'static,
) -> impl IntoView {
    let header = format!("新建服务 — 第 {} / 4 步：{}", step + 1, WIZARD_STEPS[step]);
    let save_this = move |_| on_save(());

    view! {
        <h2>{header}</h2>
        <div class="tabbar" style="margin-bottom:10px;">
            {WIZARD_STEPS.iter().enumerate().map(|(i, name)| {
                view! {
                    <span class="tab" class:active=i == step style="cursor:default;">
                        {move || format!("{} {}", if i == step { "▶" } else if i < step { "✓" } else { "○" }, name)}
                    </span>
                }
            }).collect::<Vec<_>>()}
        </div>

        {move || match step {
            0 => basic_panel(form, dirty, true).into_view(),
            1 => env_panel(form, dirty, on_preview).into_view(),
            2 => log_panel(form, dirty).into_view(),
            _ => matcher_panel(form, dirty).into_view(),
        }}

        {move || {
            let e = step_err.get();
            if e.is_empty() { view! { <div></div> }.into_view() }
            else { view! { <div class="errors">{e}</div> }.into_view() }
        }}

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

        <div class="row-btns">
            {move || if step > 0 {
                view! { <button on:click=move |_| on_prev(())>"上一步"</button> }.into_view()
            } else {
                view! { <span></span> }.into_view()
            }}
            <span class="spacer" style="flex:1"></span>
            {move || if step < 3 {
                view! { <button class="primary" on:click=move |_| on_next(())>"下一步"</button> }.into_view()
            } else {
                view! { <button class="primary" on:click=save_this>"保存并完成"</button> }.into_view()
            }}
        </div>
    }
}

// ---------- 四个标签页面板 ----------

fn basic_panel(
    form: RwSignal<FormState>,
    dirty: RwSignal<Vec<bool>>,
    is_new: bool,
) -> impl IntoView {
    let mark0 = move || dirty.update(|d| d[0] = true);
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
            <input type="text" placeholder=r#"python -m http.server 8000 --directory "C:\work\docs""#
                prop:value=move || form.get().command_line.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.command_line = v);
                    mark0();
                } />
            <span class="hint">"程序 + 参数，空格分隔，双引号包裹含空格部分；不经 shell（FR-3.6）"</span>
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
            <label>"autostart"</label>
            <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                <input type="checkbox" prop:checked=move || form.get().autostart
                    on:change=move |e| {
                        let v = event_target_checked(&e);
                        set_form_value(form, move |f| f.autostart = v);
                        mark0();
                    } />
                "应用启动时自动启动"
            </label>
            <label>"autorestart"</label>
            <select on:change=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.autorestart = v);
                    mark0();
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
                    mark0();
                } />
            <label>"startretries"</label>
            <input type="number" min="0" max="10" prop:value=move || form.get().startretries.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.startretries = v);
                    mark0();
                } />
            <label>"stopwaitsecs"</label>
            <input type="number" min="1" prop:value=move || form.get().stopwaitsecs.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.stopwaitsecs = v);
                    mark0();
                } />
            <label>"redirect_stderr"</label>
            <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                <input type="checkbox" prop:checked=move || form.get().redirect_stderr
                    on:change=move |e| {
                        let v = event_target_checked(&e);
                        set_form_value(form, move |f| f.redirect_stderr = v);
                        mark0();
                    } />
                "stderr 并入 stdout 单流"
            </label>
        </div>
    }
}

fn env_panel(
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

fn log_panel(form: RwSignal<FormState>, dirty: RwSignal<Vec<bool>>) -> impl IntoView {
    let mark2 = move || dirty.update(|d| d[2] = true);
    view! {
        <div class="field-grid">
            <label>"写入文件"</label>
            <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                <input type="checkbox" prop:checked=move || form.get().log_enabled
                    on:change=move |e| {
                        let v = event_target_checked(&e);
                        set_form_value(form, move |f| f.log_enabled = v);
                        mark2();
                    } />
                "启用文件日志（不启用 = 仅内存环形缓冲）"
            </label>
            <label>"文件路径"</label>
            <input type="text" placeholder="~/.traysupervisor/logs/xxx.log"
                prop:value=move || form.get().log_path.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.log_path = v);
                    mark2();
                } />
            <label>"单文件上限（字节）"</label>
            <input type="number" min="0" prop:value=move || form.get().log_max_bytes.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.log_max_bytes = v);
                    mark2();
                } />
            <span class="hint">"0 = 不轮转；超出后按 .1 .2 … 分块"</span>
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

fn matcher_panel(form: RwSignal<FormState>, dirty: RwSignal<Vec<bool>>) -> impl IntoView {
    let mark3 = move || dirty.update(|d| d[3] = true);
    let add_m = move |_| {
        form.update(|f| f.matchers.push(MatcherConfig {
            pattern: String::new(), template: "🌐 打开 {0}".into(), max_items: 5,
        }));
        mark3();
    };
    view! {
        <div class="hint" style="margin:0 0 8px;">"日志正则 → 托盘快捷项；留空时启用内置 HTTP URL 规则（FR-5.3）"</div>
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

/// 统一的字段写入辅助（保持 prop:value 绑定与 dirty 标记的成对出现）
fn set_form_value(f: RwSignal<FormState>, mutate: impl FnOnce(&mut FormState)) {
    f.update(mutate);
}
