//! 删除确认模态框（应用内实现）。
//!
//! 为什么不用 window.confirm / dialog 插件：dialog 插件（2.8.1）的 init 脚本把
//! window.confirm 劫持为调用已删除的 `plugin:dialog|confirm` 命令（恒 reject，
//! 表现为"点了没反应"）；`__TAURI__.dialog` 全局 API 依赖插件 api-iife 注入，
//! 与 withGlobalTauri 的注入时序耦合，同样不可靠。应用内模态框零外部依赖，
//! 开发浏览器 / tauri dev / 打包态行为一致。

use leptos::*;

/// 一次待确认操作的描述
#[derive(Clone, Debug)]
pub struct ConfirmRequest {
    /// 弹窗标题（如"删除服务"）
    pub title: String,
    /// 说明文字（含服务名 / 运行中警示）
    pub message: String,
    /// 确认按钮文字（如"删除"）
    pub confirm_label: String,
}

/// 模态确认框：`req` 为 None 时不渲染；确认时回调 `on_confirm`（组件内部随即清空 req）。
pub fn confirm_dialog(
    req: RwSignal<Option<ConfirmRequest>>,
    on_confirm: impl Fn() + Copy + 'static,
) -> impl IntoView {
    move || {
        let Some(r) = req.get() else {
            return view! { <div></div> }.into_view();
        };
        let on_cancel = move |_| req.set(None);
        // 先回调业务动作、再关弹窗：业务方可能依赖 req 状态判断本次确认来源
        let on_ok = move |_| {
            on_confirm();
            req.set(None);
        };
        view! {
            <div
                style="position:fixed;inset:0;background:rgba(0,0,0,.4);display:flex;align-items:center;justify-content:center;z-index:1000;"
                on:click=on_cancel
            >
                <div
                    style="background:var(--bg);border:1px solid var(--line);border-radius:8px;padding:16px 18px;max-width:400px;min-width:300px;box-shadow:0 8px 30px rgba(0,0,0,.2);"
                    on:click=|e: ev::MouseEvent| e.stop_propagation()
                >
                    <div style="font-weight:600;font-size:14px;margin-bottom:8px;">{r.title}</div>
                    <div style="color:var(--muted);line-height:1.7;font-size:13px;">{r.message}</div>
                    <div style="display:flex;gap:8px;justify-content:flex-end;margin-top:14px;">
                        <button on:click=on_cancel>"取消"</button>
                        <button class="danger" on:click=on_ok>{r.confirm_label}</button>
                    </div>
                </div>
            </div>
        }
        .into_view()
    }
}
