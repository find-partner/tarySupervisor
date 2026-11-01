//! 关于栏（FR-2.14 / FR-2.15）：设置窗口底部常驻——应用名 + 版本号、平台/构建类型、
//! 仓库链接与新版本提示。数据来自 `get_app_info`（包元数据编译期注入，前端零硬编码，
//! HLD D-18）；更新检查（FR-2.15）经 GitHub Releases API 直连，见 update.rs；
//! 外链均经 `open_external_url` 打开（http/https 校验同 FR-5.6），不用 `<a href>`
//! （避免 WebView 内导航劫持窗口，HLD §3.6）。

use leptos::*;

use crate::bridge;
use crate::dto::AppInfoDto;

use super::update;

pub fn about_bar() -> impl IntoView {
    let info = create_rw_signal::<Option<AppInfoDto>>(None);
    let update = create_rw_signal::<Option<update::UpdateCheck>>(None);
    spawn_local(async move {
        let v = bridge::invoke_t::<AppInfoDto>("get_app_info", None).await;
        // 更新检查（FR-2.15）：关于栏加载即查（24h 节流），失败静默
        let found = update::check(&v).await;
        update.set(found);
        info.set(Some(v));
    });
    let open_url = move |url: String| {
        spawn_local(async move {
            let _ = bridge::invoke("open_external_url", Some(bridge::invoke_args_url(&url))).await;
        });
    };
    view! {
        <div class="about-bar">
            {move || {
                let v = info.get()?;
                let repo_text = v
                    .repository
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .to_string();
                let repo_for_click = repo_text.clone();
                let upd = update.get();
                Some(view! {
                    <span class="muted">
                        {v.name}" v"{v.version}" · "{v.os}"-"{v.arch}" · "{v.profile}
                    </span>
                    <span
                        class="link"
                        title=v.repository.clone()
                        on:click=move |_| open_url(repo_for_click.clone())
                    >{repo_text}</span>
                    {upd.map(|u| {
                        let url = u.release_url.clone();
                        view! {
                            <span
                                class="link"
                                title=format!("打开 Release 页：{}", u.release_url)
                                on:click=move |_| open_url(url.clone())
                            >"新版本 "{u.tag}" 可用 →"</span>
                        }
                    })}
                })
            }}
        </div>
    }
}
