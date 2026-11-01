//! 关于栏（FR-2.14）：设置窗口底部常驻——应用名 + 版本号、平台/构建类型、仓库链接。
//! 数据来自 `get_app_info`（包元数据编译期注入，前端零硬编码，HLD D-18）；
//! 仓库链接经 `open_external_url` 打开（http/https 校验同 FR-5.6），不用 `<a href>`
//! （避免 WebView 内导航劫持窗口，HLD §3.6）。

use leptos::*;

use crate::bridge;
use crate::dto::AppInfoDto;

pub fn about_bar() -> impl IntoView {
    let info = create_rw_signal::<Option<AppInfoDto>>(None);
    spawn_local(async move {
        info.set(Some(bridge::invoke_t::<AppInfoDto>("get_app_info", None).await));
    });
    let open_repo = move |_| {
        let Some(v) = info.get_untracked() else { return };
        spawn_local(async move {
            let _ = bridge::invoke(
                "open_external_url",
                Some(bridge::invoke_args_url(&v.repository)),
            )
            .await;
        });
    };
    view! {
        <div class="about-bar">
            {move || {
                info.get().map(|v| {
                    // 仓库展示去掉协议前缀，title 保留完整 URL
                    let repo_text = v
                        .repository
                        .trim_start_matches("https://")
                        .trim_start_matches("http://")
                        .to_string();
                    let repo_title = v.repository.clone();
                    view! {
                        <span class="muted">
                            {v.name}" v"{v.version}" · "{v.os}"-"{v.arch}" · "{v.profile}
                        </span>
                        <span class="link" title=repo_title on:click=open_repo>{repo_text}</span>
                    }
                })
            }}
        </div>
    }
}
