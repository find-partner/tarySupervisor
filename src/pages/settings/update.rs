//! 更新检查（FR-2.15 / HLD D-19）：GitHub Releases 版本比对，只做「检查 + 提醒」。
//! 前端直连 api.github.com（CORS 开放，无后端命令、零新增依赖）；24h localStorage
//! 节流；失败静默（不影响关于栏其余内容）。完整自动更新（tauri-plugin-updater，
//! 下载并静默安装）需更新签名密钥与 CI secrets，列为二期（SRS §8）。

use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;

use crate::bridge;
use crate::dto::AppInfoDto;

/// 检查结果（有新版本时关于栏展示并跳转）
#[derive(Debug, Clone)]
pub struct UpdateCheck {
    /// 最新发布版本号（含 v 前缀，如 v0.5.1）
    pub tag: String,
    /// Release 页 URL（经 open_external_url 打开）
    pub release_url: String,
}

impl UpdateCheck {
    fn if_newer_than(self, current: &str) -> Option<Self> {
        version_newer(&self.tag, current).then_some(self)
    }
}

/// localStorage 缓存条目（节流：TTL 内不重复请求 API）
#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    tag: String,
    release_url: String,
    ts_ms: f64,
}

const CACHE_KEY: &str = "tsv_update_check";
const CACHE_TTL_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;

/// 语义化版本比较：latest 严格大于 current 时为真。
/// 容忍 v 前缀与 pre-release/build 后缀（`0.6.0-rc.1` 数值上视同 `0.6.0`），逐段数值比较。
pub fn version_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .trim_start_matches('v')
            .split('.')
            .map(|p| {
                p.split(|c| c == '-' || c == '+')
                    .next()
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    };
    let (l, c) = (parse(latest), parse(current));
    for i in 0..3 {
        let lv = l.get(i).copied().unwrap_or(0);
        let cv = c.get(i).copied().unwrap_or(0);
        if lv != cv {
            return lv > cv;
        }
    }
    false
}

/// 从仓库 URL（AppInfo.repository，D-18 单一来源）提取 `owner/repo`；非 GitHub 返回 None
pub fn github_repo(repository: &str) -> Option<String> {
    let rest = repository
        .trim_start_matches("https://github.com/")
        .trim_start_matches("http://github.com/")
        .trim_end_matches(".git")
        .trim_matches('/');
    (!rest.is_empty() && rest.split('/').count() == 2).then(|| rest.to_string())
}

/// 检查更新：TTL 内复用缓存；否则请求 Releases API（`/releases/latest` 已排除
/// draft/prerelease）。任何失败（非 GitHub 仓库、限流、无发布版）返回 None 且不写缓存。
pub async fn check(info: &AppInfoDto) -> Option<UpdateCheck> {
    let repo = github_repo(&info.repository)?;
    let now = js_sys::Date::now();
    if let Some(c) = read_cache() {
        if now - c.ts_ms < CACHE_TTL_MS {
            return UpdateCheck {
                tag: c.tag,
                release_url: c.release_url,
            }
            .if_newer_than(&info.version);
        }
    }
    let json = bridge::fetch_json(&format!(
        "https://api.github.com/repos/{repo}/releases/latest"
    ))
    .await
    .ok()?;
    let tag = js_get(&json, "tag_name");
    let release_url = js_get(&json, "html_url");
    if tag.is_empty() || release_url.is_empty() {
        return None;
    }
    write_cache(&CacheEntry {
        tag: tag.clone(),
        release_url: release_url.clone(),
        ts_ms: now,
    });
    UpdateCheck { tag, release_url }.if_newer_than(&info.version)
}

fn js_get(v: &JsValue, key: &str) -> String {
    js_sys::Reflect::get(v, &JsValue::from_str(key))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default()
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn read_cache() -> Option<CacheEntry> {
    let raw = storage()?.get_item(CACHE_KEY).ok().flatten()?;
    serde_json::from_str(&raw).ok()
}

fn write_cache(c: &CacheEntry) {
    if let Ok(raw) = serde_json::to_string(c) {
        if let Some(s) = storage() {
            let _ = s.set_item(CACHE_KEY, &raw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(version_newer("v0.6.0", "0.5.0"));
        assert!(version_newer("0.5.1", "v0.5.0"));
        assert!(version_newer("1.0.0", "0.9.9"));
        assert!(!version_newer("v0.5.0", "0.5.0"));
        assert!(!version_newer("0.5.0", "0.5.1"));
        assert!(!version_newer("0.4.9", "0.5.0"));
        // pre-release/build 后缀不参与数值比较（0.6.0-rc.1 视同 0.6.0）
        assert!(!version_newer("0.6.0-rc.1", "0.6.0"));
        // 段数不齐按 0 补齐
        assert!(version_newer("v1.0", "0.9.9"));
    }

    #[test]
    fn repo_parse() {
        assert_eq!(
            github_repo("https://github.com/find-partner/tarySupervisor").as_deref(),
            Some("find-partner/tarySupervisor")
        );
        assert_eq!(github_repo("https://github.com/a/b.git").as_deref(), Some("a/b"));
        assert_eq!(github_repo("https://gitlab.com/a/b"), None);
        assert_eq!(github_repo("https://github.com/"), None);
        assert_eq!(github_repo(""), None);
    }
}
