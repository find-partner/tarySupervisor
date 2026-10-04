//! 应用入口与窗口路由：每个 Tauri 窗口以 hash 定位页面
//! （#/settings | #/log/<id> | #/term/<id>），窗口内不再导航。

use crate::pages;
use leptos::*;
use wasm_bindgen::JsCast;

#[component]
fn App() -> impl IntoView {
    let hash = web_sys::window()
        .map(|w| w.location().hash().unwrap_or_default())
        .unwrap_or_default();

    if let Some(id) = hash.strip_prefix("#/log/") {
        view! { <pages::log_page::LogPage id=id.to_string() /> }
    } else if let Some(id) = hash.strip_prefix("#/term/") {
        view! { <pages::term_page::TermPage id=id.to_string() /> }
    } else {
        view! { <pages::settings::SettingsPage /> }
    }
}

pub fn run() {
    console_error_panic_hook::set_once();

    // 挂载到 index.html 预置的 #app（height:100%）。此前用 mount_to_body：UI 成为
    // 占满整屏的 #app 的后继兄弟节点，被推到视口之外，导致设置/日志/终端窗口白屏。
    let app_el = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("app"))
        .map(|el| el.unchecked_into::<web_sys::HtmlElement>());

    match app_el {
        Some(el) => mount_to(el, || view! { <App /> }),
        None => mount_to_body(|| view! { <App /> }),
    }
}
