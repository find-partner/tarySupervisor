//! 新建向导（FR-2.9）：四步引导，前两步可跳过，最后一步整体保存。

use leptos::*;

use super::basic::basic_panel;
use super::panels::{env_panel, log_panel, matcher_panel};
use super::form::FormState;

const WIZARD_STEPS: [&str; 4] = ["基本设置", "环境变量（可跳过）", "日志（可跳过）", "快捷项（可跳过）"];

#[allow(clippy::too_many_arguments)]
pub fn wizard_mode(
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
