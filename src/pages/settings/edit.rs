//! 编辑模式（FR-2.8 四标签页 + FR-2.10 保存后"需重启生效"提示 +
//! FR-2.11 标签栏上方服务操作栏）。

use leptos::*;

use super::basic::basic_panel;
use super::ctrlbar::ctrl_bar;
use super::panels::{env_panel, log_panel, matcher_panel};
use super::form::FormState;

const TAB_NAMES: [&str; 4] = ["基本设置", "环境变量", "日志", "快捷项"];

#[allow(clippy::too_many_arguments)]
pub fn edit_mode(
    form: RwSignal<FormState>,
    tab: RwSignal<usize>,
    dirty: RwSignal<Vec<bool>>,
    errors: RwSignal<Vec<String>>,
    banner: RwSignal<Vec<String>>,
    preview: RwSignal<Option<Result<String, String>>>,
    restart_hint: RwSignal<Option<(String, String)>>,
    selected_id: RwSignal<Option<String>>,
    ctrl_err: RwSignal<String>,
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

        // 服务操作栏（FR-2.11）：状态 + 启动/停止/重启 + 查看日志/终端视图
        {ctrl_bar(selected_id, ctrl_err)}

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
