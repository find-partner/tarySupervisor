//! 设置窗口（FR-2 / SRS FR-2A）主组件：左侧服务列表 + 右侧三态编辑区——
//! 空态引导（FR-2.7，empty.rs）/ 编辑模式（FR-2.8，edit.rs）/
//! 新建向导（FR-2.9，wizard.rs）；表单模型见 form.rs，四个标签页面板见
//! basic.rs 与 panels.rs。

mod basic;
mod confirm;
mod ctrlbar;
mod edit;
mod empty;
mod form;
mod panels;
mod wizard;

use leptos::*;

use crate::bridge;
use crate::dto::{ConfigInvalidEv, ServiceBriefOut};
use crate::store;

use confirm::{ConfirmRequest, confirm_dialog};
use form::{blank_form, form_from_brief, input_from_form, merged_for_save};

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
    // 共享服务数据源（FR-1.10）：与托盘菜单同源（list_services 快照 + 事件刷新），
    // 列表与编辑页操作栏（FR-2.11）共用，保证状态一致
    let list = store::ensure();
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
    // 删除确认模态框的待确认请求（见 confirm.rs：不用 window.confirm / dialog 插件）
    let pending_del = create_rw_signal::<Option<ConfirmRequest>>(None);
    // 操作栏启停命令的运行时错误（如「当前为 RUNNING，无需启动」，FR-2.11）
    let ctrl_err = create_rw_signal(String::new());

    // 配置热更/校验失败横幅（FR-2.6；数据刷新由 store 负责）
    spawn_local(async move {
        let _ = bridge
            ::listen("config-changed", move |_| {
                banner.set(Vec::new());
            })
            .await;
        let _ = bridge
            ::listen("config-invalid", move |payload| {
                if let Ok(ev) = serde_wasm_bindgen::from_value::<ConfigInvalidEv>(payload) {
                    banner.set(ev.errors);
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
    let save_from = move |src: form::FormState, from_wizard: bool| {
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
                store::refresh().await;
            });
        }
    };

    // 「删除此服务」→ 弹应用内确认框；确认后 do_delete 真正执行
    let del = move |_| {
        let Some(id) = selected_id.get() else { return };
        let brief = list.get().into_iter().find(|b| b.config.id == id.clone());
        let running = brief
            .as_ref()
            .map(|b| b.state == "RUNNING" || b.state == "STARTING")
            .unwrap_or(false);
        let name = brief
            .map(|b| b.display_name().to_string())
            .unwrap_or_else(|| id.clone());
        let who = if name.is_empty() { id.clone() } else { name };
        let message = if running {
            format!("服务 {who} 正在运行，删除前将先停止。此操作同时从配置文件移除该服务。")
        } else {
            format!("将删除服务 {who} 并从配置文件移除。该操作不可撤销。")
        };
        pending_del.set(Some(ConfirmRequest {
            title: "删除服务".into(),
            message,
            confirm_label: "确认删除".into(),
        }));
    };
    let do_delete = move || {
        let Some(id) = selected_id.get_untracked() else { return };
        pending_del.set(None);
        spawn_local(async move {
            match bridge::invoke_errors("delete_service", Some(bridge::invoke_args_id(&id))).await {
                Ok(()) => {
                    selected_id.set(None);
                    form.set(blank_form());
                    loaded.set(blank_form());
                    mode.set(Mode::Empty);
                    errors.set(Vec::new());
                    dirty.set(vec![false; 4]);
                    restart_hint.set(None);
                    store::refresh().await;
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
        Mode::Empty => empty::empty_state().into_view(),
        Mode::Edit => edit::edit_mode(
            form, tab, dirty, errors, banner, preview, restart_hint,
            selected_id, ctrl_err, save_tab, restart_now, del, do_preview,
        ).into_view(),
        Mode::Wizard(step) => wizard::wizard_mode(
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

        // 应用内确认弹窗（覆盖层，替代 window.confirm / dialog 插件）
        {confirm_dialog(pending_del, do_delete)}
    }
}
