//! 应用入口与窗口路由：每个 Tauri 窗口以 hash 定位页面
//! （#/settings | #/log/<id> | #/term/<id>），窗口内不再导航。

use crate::pages;
use leptos::*;

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
    mount_to_body(|| view! { <App /> });
}
