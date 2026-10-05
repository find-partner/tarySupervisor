//! 日志正则匹配与快捷菜单项（FR-5 / HLD §3.3 Matcher 引擎）。
//!
//! - 对每条新到达的日志行流式匹配（合并器之后，与存储解耦，D-6）；
//! - 命中后按模板渲染菜单文案，`{0} {1}` 引用捕获组；
//! - 去重：同一匹配值只保留一项，以匹配结果哈希为键；
//! - 每条 matcher 上限 max_items，新命中按 LRU 淘汰；
//! - 打开链接前仅允许 http/https（FR-5.6）。

use crate::config::MatcherConfig;
use crate::events::MatchItem;
use regex::Regex;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// 生成快捷项 hash（去重键 + 托盘菜单 ID 编码）。
/// 截断到 53 bit：跨 IPC 的整数必须是 JS 安全整数（DEV-CONVENTIONS §2），
/// 完整 u64 会让前端 serde-wasm-bindgen 报 floating point 反序列化错。
/// 截断仅影响碰撞概率（≤10 项/服务，2^53 下约 5e-15），hash 不落盘、可随版本变化。
fn short_hash(idx: usize, label: &str, url: &Option<String>) -> u64 {
    let mut hasher = DefaultHasher::new();
    idx.hash(&mut hasher);
    label.hash(&mut hasher);
    url.hash(&mut hasher);
    hasher.finish() & ((1 << 53) - 1)
}

/// 已编译 Matcher（含 LRU 快捷项集合）
pub struct MatcherState {
    matchers: Vec<CompiledMatcher>,
    /// 全部快捷项，队首最新（LRU：命中移到队首，超限从队尾淘汰）
    items: Vec<Item>,
}

struct Item {
    matcher_idx: usize,
    inner: MatchItem,
}

struct CompiledMatcher {
    regex: Regex,
    template: String,
    max_items: u32,
}

impl MatcherState {
    /// 从配置构建；空配置 = 无任何匹配规则（内置 URL 规则不再隐式启用，
    /// 由新建服务时的 UI 预填，FR-5.3）。
    /// （保存/加载时已校验可编译，这里对失败容忍：跳过并记日志）
    pub fn new(cfgs: &[MatcherConfig]) -> Self {
        let mut matchers = Vec::new();
        for c in cfgs {
            match Regex::new(&c.pattern) {
                Ok(regex) => matchers.push(CompiledMatcher {
                    regex,
                    template: if c.template.is_empty() {
                        "🌐 打开 {0}".to_string()
                    } else {
                        c.template.clone()
                    },
                    max_items: c.max_items.clamp(1, 10),
                }),
                Err(e) => log::warn!("matcher 编译失败已跳过：{e}"),
            }
        }
        Self { matchers, items: Vec::new() }
    }

    /// 配置热更后整体替换规则（旧快捷项一并清空）
    pub fn replace(&mut self, cfgs: &[MatcherConfig]) {
        *self = Self::new(cfgs);
    }

    /// 对一行日志执行全部 matcher；返回本次是否产生变化。
    pub fn feed_line(&mut self, text: &str) -> bool {
        let mut changed = false;
        for (idx, m) in self.matchers.iter().enumerate() {
            for caps in m.regex.captures_iter(text) {
                // 捕获组 0 = 整体匹配，1..n = 分组
                let mut groups: Vec<String> = Vec::new();
                for gi in 0..caps.len() {
                    groups.push(caps.get(gi).map(|g| g.as_str().to_string()).unwrap_or_default());
                }
                let label = render_template(&m.template, &groups);
                let url = groups
                    .iter()
                    .map(|g| trim_url(g))
                    .find(|g| g.starts_with("http://") || g.starts_with("https://"));

                let hash = short_hash(idx, &label, &url);

                // 去重：已存在 → 触碰（移到队首）
                if let Some(pos) = self.items.iter().position(|i| i.inner.hash == hash) {
                    let item = self.items.remove(pos);
                    self.items.insert(0, item);
                    continue;
                }
                self.items.insert(
                    0,
                    Item { matcher_idx: idx, inner: MatchItem { hash, label, url } },
                );
                changed = true;
            }
        }
        // 每条 matcher 各自按 max_items 做 LRU 淘汰（FR-5.4）
        for idx in 0..self.matchers.len() {
            let max = self.matchers[idx].max_items as usize;
            let mut count = self.items.iter().filter(|i| i.matcher_idx == idx).count();
            if count > max {
                // 从队尾（最旧）开始移除
                let mut scan = self.items.len();
                while scan > 0 && count > max {
                    scan -= 1;
                    if self.items[scan].matcher_idx == idx {
                        self.items.remove(scan);
                        count -= 1;
                    }
                }
                changed = true;
            }
        }
        changed
    }

    pub fn items(&self) -> Vec<MatchItem> {
        self.items.iter().map(|i| i.inner.clone()).collect()
    }

    /// 服务重启后清空旧快捷项（FR-5.5：旧输出不再展示）
    pub fn clear(&mut self) {
        self.items.clear();
    }
}

/// 模板渲染：`{n}` 替换为捕获组 n（缺失组替换为空串），其余字面保留。
fn render_template(template: &str, groups: &[String]) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'}' && j > i + 1 {
                let n: usize = template[i + 1..j].parse().unwrap_or(0);
                if let Some(g) = groups.get(n) {
                    out.push_str(g);
                }
                i = j + 1;
                continue;
            }
        }
        // 多字节字符按 char 推进
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&template[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// URL 尾部清理：正则 `\S+` 常把句末标点一并捕获。
fn trim_url(u: &str) -> String {
    u.trim_end_matches(['.', ',', ';', ':', '!', '?', '"', '\'', ')', ']', '}', '>'])
        .to_string()
}

/// URL scheme 校验（FR-5.6）：仅 http/https。
pub fn is_safe_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pattern: &str, template: &str, max_items: u32) -> MatcherConfig {
        MatcherConfig { pattern: pattern.into(), template: template.into(), max_items }
    }

    /// 内置 URL 规则（FR-5.3）：新建服务时由 UI 预填，测试里显式传入。
    fn url_matcher_state() -> MatcherState {
        MatcherState::new(&[m(
            crate::config::BUILTIN_URL_PATTERN,
            crate::config::BUILTIN_URL_TEMPLATE,
            5,
        )])
    }

    #[test]
    fn empty_rules_match_nothing() {
        // 无规则即无快捷项；内置 URL 规则不再隐式启用（FR-5.3 语义修正）
        let mut st = MatcherState::new(&[]);
        assert!(!st.feed_line("Serving at http://127.0.0.1:8000 "));
        assert!(st.items().is_empty());
    }

    #[test]
    fn builtin_url_hit() {
        let mut st = url_matcher_state();
        assert!(st.feed_line("Serving at http://127.0.0.1:8000 "));
        let items = st.items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "🌐 打开 http://127.0.0.1:8000");
        assert_eq!(items[0].url.as_deref(), Some("http://127.0.0.1:8000"));
    }

    #[test]
    fn dedupes_by_result_hash() {
        let mut st = url_matcher_state();
        st.feed_line("go http://a.example");
        st.feed_line("go http://a.example again");
        st.feed_line("go http://a.example");
        assert_eq!(st.items().len(), 1);
        st.feed_line("go http://b.example");
        assert_eq!(st.items().len(), 2);
    }

    #[test]
    fn hash_stays_within_js_safe_integer() {
        // DEV-CONVENTIONS §2：跨 IPC 整数 ≤ 2^53-1，否则前端反序列化失败
        for i in 0..1000usize {
            let h = short_hash(i, &format!("label {i}"), &Some(format!("http://x/{i}")));
            assert!(h < (1u64 << 53), "hash {h} 超出 JS 安全整数范围");
        }
    }

    #[test]
    fn lru_eviction_per_matcher() {
        let mut st = MatcherState::new(&[m(r"item-(\d)", "数字 {0}", 2)]);
        st.feed_line("item-1 item-2 item-3");
        let items = st.items();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "数字 item-3", "最新在前");
        st.feed_line("item-2");
        // item-2 被触碰后仍在（LRU 复活），item-1 被淘汰
        let labels: Vec<_> = st.items().iter().map(|i| i.label.clone()).collect();
        assert_eq!(labels, vec!["数字 item-2", "数字 item-3"]);
    }

    #[test]
    fn capture_groups_in_template() {
        // {0} = 整体匹配，{1} = 第一个捕获组（标准正则语义）
        let mut st = MatcherState::new(&[m(r"Listening on port (\d+)", "端口 {1}", 5)]);
        st.feed_line("Listening on port 3000");
        let items = st.items();
        assert_eq!(items[0].label, "端口 3000");
        assert_eq!(items[0].url, None, "非 URL 项");
    }

    #[test]
    fn clear_on_restart() {
        let mut st = url_matcher_state();
        st.feed_line("http://x");
        assert!(!st.items().is_empty());
        st.clear();
        assert!(st.items().is_empty());
    }

    #[test]
    fn url_trailing_punctuation_trimmed() {
        let mut st = url_matcher_state();
        st.feed_line("see https://example.com/docs.");
        let url = st.items()[0].url.clone().unwrap();
        assert_eq!(url, "https://example.com/docs");
    }

    #[test]
    fn safe_url_check() {
        assert!(is_safe_url("https://a.b"));
        assert!(is_safe_url("HTTP://a.b"));
        assert!(!is_safe_url("file:///etc/passwd"));
        assert!(!is_safe_url("javascript:alert(1)"));
        assert!(!is_safe_url("httpsjavascript:"));
    }

    #[test]
    fn template_keeps_literal_braces_on_missing_group() {
        let mut st = MatcherState::new(&[m(r"v=(\d+)", "值 {1} {5} {x}", 5)]);
        st.feed_line("v=7");
        assert_eq!(st.items()[0].label, "值 7  {x}");
    }
}
