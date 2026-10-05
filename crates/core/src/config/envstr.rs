//! environment 解析/序列化（CONFIG-SPEC §4）与 `${VAR}` 变量展开（FR-2.3 / HLD D-4）。
//!
//! 文件格式：`environment=KEY="value", KEY2="!value2"@windows`
//! - value 必须双引号包裹；内部 `"` 写作 `\"`；内部逗号无需转义；
//! - 前缀 `!` 开启替换（expand=true）；无前缀 = 字面量直传；
//! - 引号后可选 `@平台` 行级平台标签（FR-2.13）；`@all` 或无标签 = 全部平台；
//! - 替换规则：仅支持 `${VAR}`，先查本服务 env 表中排在其前面的条目，再查系统环境；
//!   未定义 → 错误；不支持转义、嵌套与递归展开。

use super::EnvEntry;
use std::collections::BTreeMap;

/// environment= 值解析为有序 KV 表。
pub fn parse_environment(s: &str) -> Result<Vec<EnvEntry>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut entries = Vec::new();
    let mut i = 0;
    let n = chars.len();

    let skip_ws = |i: &mut usize| {
        while *i < n && chars[*i].is_whitespace() {
            *i += 1;
        }
    };

    loop {
        skip_ws(&mut i);
        if i >= n {
            break;
        }
        // 键：读到 '='；不允许空白、引号、逗号
        let key_start = i;
        while i < n
            && chars[i] != '='
            && !chars[i].is_whitespace()
            && chars[i] != '"'
            && chars[i] != ','
        {
            i += 1;
        }
        if i == key_start {
            return Err(format!(
                "environment 第 {} 项缺少合法键名",
                entries.len() + 1
            ));
        }
        let key: String = chars[key_start..i].iter().collect();
        skip_ws(&mut i);
        if i >= n || chars[i] != '=' {
            return Err(format!("environment 键 {key} 后缺少 '='"));
        }
        i += 1;
        skip_ws(&mut i);
        if i >= n || chars[i] != '"' {
            return Err(format!("environment 键 {key} 的值必须用双引号包裹"));
        }
        i += 1;
        // 值：读到闭合引号；`\"` → `"`，`\\` → `\`，`\x` → 字面 `\x`
        let mut value = String::new();
        loop {
            if i >= n {
                return Err(format!("environment 键 {key} 的值引号未闭合"));
            }
            let c = chars[i];
            if c == '\\' && i + 1 < n {
                let next = chars[i + 1];
                if next == '"' || next == '\\' {
                    value.push(next);
                } else {
                    value.push('\\');
                    value.push(next);
                }
                i += 2;
            } else if c == '\\' {
                value.push('\\');
                i += 1;
            } else if c == '"' {
                i += 1;
                break;
            } else {
                value.push(c);
                i += 1;
            }
        }
        skip_ws(&mut i);
        // 行级平台标签（FR-2.13）：引号后可选 `@平台`；`@all` 归一化为全部平台
        let platform = if i < n && chars[i] == '@' {
            i += 1;
            let tag_start = i;
            while i < n && chars[i] != ',' && !chars[i].is_whitespace() {
                i += 1;
            }
            let tag: String = chars[tag_start..i].iter().collect();
            if tag.is_empty() {
                return Err(format!(
                    "environment 键 {key} 的平台标签为空（@ 后应有平台名）"
                ));
            }
            if tag.eq_ignore_ascii_case("all") {
                None
            } else {
                Some(tag)
            }
        } else {
            None
        };
        skip_ws(&mut i);
        if i < n {
            if chars[i] != ',' {
                return Err(format!("environment 键 {key} 后应为 ','、'@平台' 或结尾"));
            }
            i += 1;
        }
        let (expand, value) = match value.strip_prefix('!') {
            Some(rest) => (true, rest.to_string()),
            None => (false, value),
        };
        entries.push(EnvEntry {
            key,
            value,
            expand,
            platform,
        });
    }
    Ok(entries)
}

/// 序列化为 environment= 的值。转义与解析规则对称且最小化：
/// `"` → `\"`；`\` 仅在会被误读为转义时（后跟 `"`/`\`/空白/末尾）双写，其余保留可读；
/// 带平台标签的条目在引号后写 `@平台`。
pub fn serialize_environment(entries: &[EnvEntry]) -> String {
    entries
        .iter()
        .map(|e| {
            let v = if e.expand {
                format!("!{}", e.value)
            } else {
                e.value.clone()
            };
            let tag = e
                .platform
                .as_ref()
                .map(|p| format!("@{p}"))
                .unwrap_or_default();
            format!("{}=\"{}\"{}", e.key, escape_quoted(&v), tag)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn escape_quoted(v: &str) -> String {
    let chars: Vec<char> = v.chars().collect();
    let mut out = String::with_capacity(v.len());
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '"' => {
                out.push('\\');
                out.push('"');
            }
            '\\' => {
                let next_is_special = chars
                    .get(i + 1)
                    .is_none_or(|&n| n == '"' || n == '\\' || n.is_whitespace());
                if next_is_special {
                    out.push('\\');
                }
                out.push('\\');
            }
            c => out.push(c),
        }
    }
    out
}

/// 展开单个值中的 `${VAR}`（D-4：仅此一种语法）。
///
/// `lookup` 依次被调用：先本服务 env 表中排在前面的条目，再系统环境。
/// 未定义变量返回 Err（含变量名）。
pub fn expand_env_value(
    value: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
            let mut j = i + 2;
            while j < chars.len() && chars[j] != '}' {
                j += 1;
            }
            if j >= chars.len() {
                return Err("变量引用缺少闭合 '}'".to_string());
            }
            let var: String = chars[i + 2..j].iter().collect();
            if var.is_empty() {
                return Err("空的变量引用 ${}".to_string());
            }
            match lookup(&var) {
                Some(v) => out.push_str(&v),
                None => return Err(format!("引用了未定义的环境变量 ${{{var}}}")),
            }
            i = j + 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    Ok(out)
}

/// 已构建部分的查找上下文：先查表中排在前面的条目，再查系统环境。
pub struct EnvLookup<'a> {
    pub local: &'a BTreeMap<String, String>,
    pub system: &'a dyn Fn(&str) -> Option<String>,
}

impl<'a> EnvLookup<'a> {
    pub fn get(&self, var: &str) -> Option<String> {
        if let Some(v) = self.local.get(var) {
            return Some(v.clone());
        }
        (self.system)(var)
    }
}

/// 按表顺序构建本服务最终环境变量表（HLD §3.1 resolve_env）。
///
/// expand=true 的条目以「当前已构建 map + 系统环境」为上下文执行替换；
/// expand=false 的条目字面量直传。任一条目失败返回 Err（逐条累积所有错误）。
pub fn resolve_env(
    entries: &[EnvEntry],
    system: &dyn Fn(&str) -> Option<String>,
) -> Result<BTreeMap<String, String>, Vec<String>> {
    let mut map = BTreeMap::new();
    let mut errors = Vec::new();
    for e in entries {
        if e.value.is_empty() && e.expand {
            errors.push(format!("键 {}：开启替换的值为空", e.key));
            continue;
        }
        let value = if e.expand {
            let local_at = map.clone();
            let lookup = EnvLookup {
                local: &local_at,
                system,
            };
            match expand_env_value(&e.value, &|v| lookup.get(v)) {
                Ok(v) => v,
                Err(err) => {
                    errors.push(format!("键 {}：{}", e.key, err));
                    continue;
                }
            }
        } else {
            e.value.clone()
        };
        map.insert(e.key.clone(), value);
    }
    if errors.is_empty() {
        Ok(map)
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spec_example() {
        let entries = parse_environment(
            r#"APP_HOME="C:\opt\myapp", PATH_EXTRA="!${APP_HOME}\bin", GREETING="hello, world""#,
        )
        .unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].key, "APP_HOME");
        assert_eq!(entries[0].value, r"C:\opt\myapp");
        assert!(!entries[0].expand);
        assert_eq!(entries[1].key, "PATH_EXTRA");
        assert_eq!(entries[1].value, r"${APP_HOME}\bin");
        assert!(entries[1].expand);
        assert_eq!(entries[2].value, "hello, world");
    }

    #[test]
    fn parses_escaped_quote_and_comma() {
        let entries = parse_environment(r#"A="say \"hi\", ok", B="back\\slash""#).unwrap();
        assert_eq!(entries[0].value, r#"say "hi", ok"#);
        assert_eq!(entries[1].value, r"back\slash");
    }

    #[test]
    fn errors_on_missing_quotes() {
        assert!(parse_environment(r#"A=hello"#).is_err());
        assert!(parse_environment(r#"A="unterminated"#).is_err());
        assert!(parse_environment(r#"="nokey""#).is_err());
        assert!(parse_environment(r#"A="x" B="y""#).is_err());
    }

    #[test]
    fn serialize_roundtrip() {
        let entries = vec![
            EnvEntry {
                key: "APP_HOME".into(),
                value: r"C:\opt\myapp".into(),
                expand: false,
                platform: None,
            },
            EnvEntry {
                key: "PATH_EXTRA".into(),
                value: r"${APP_HOME}\bin".into(),
                expand: true,
                platform: None,
            },
            EnvEntry {
                key: "MSG".into(),
                value: "he said \"hi\", ok".into(),
                expand: false,
                platform: None,
            },
            EnvEntry {
                key: "TRAIL".into(),
                value: "end\\".into(),
                expand: false,
                platform: None,
            },
        ];
        let s = serialize_environment(&entries);
        let back = parse_environment(&s).unwrap();
        assert_eq!(back, entries, "serialized = {s}");
    }

    // ---------- 行级平台标签（FR-2.13）----------

    #[test]
    fn parses_platform_tags() {
        let entries = parse_environment(
            r#"PORT="3000", LOG_DIR="!${LOCALAPPDATA}\logs"@windows, EXTRA="/opt/bin" @macos"#,
        )
        .unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].platform, None);
        assert_eq!(entries[1].platform.as_deref(), Some("windows"));
        assert_eq!(entries[1].value, r"${LOCALAPPDATA}\logs");
        assert!(entries[1].expand);
        assert_eq!(entries[2].platform.as_deref(), Some("macos"));
    }

    #[test]
    fn tag_all_normalizes_to_none() {
        let entries = parse_environment(r#"A="1"@all"#).unwrap();
        assert_eq!(entries[0].platform, None);
    }

    #[test]
    fn tag_serialize_roundtrip() {
        let entries = vec![
            EnvEntry {
                key: "A".into(),
                value: "shared".into(),
                expand: false,
                platform: None,
            },
            EnvEntry {
                key: "B".into(),
                value: "win only".into(),
                expand: false,
                platform: Some("windows".into()),
            },
            EnvEntry {
                key: "C".into(),
                value: "!${A}/m".into(),
                expand: true,
                platform: Some("home-mac".into()),
            },
        ];
        let s = serialize_environment(&entries);
        assert_eq!(parse_environment(&s).unwrap(), entries, "serialized = {s}");
        assert!(s.contains(r#"B="win only"@windows"#), "{s}");
    }

    #[test]
    fn empty_tag_rejected() {
        assert!(parse_environment(r#"A="1"@, B="2""#).is_err());
        assert!(parse_environment(r#"A="1"@ "#).is_err());
    }

    #[test]
    fn hand_written_and_serialized_both_parse() {
        // 手写风格（单反斜杠）与序列化风格（双反斜杠）都必须能还原
        for src in [r#"K="C:\opt\myapp""#, r#"K="C:\\opt\\myapp""#] {
            let e = parse_environment(src).unwrap();
            assert_eq!(e[0].value, r"C:\opt\myapp", "src = {src}");
        }
    }

    #[test]
    fn expands_from_local_then_system() {
        let entries = vec![
            EnvEntry {
                key: "A".into(),
                value: "1".into(),
                expand: false,
                platform: None,
            },
            EnvEntry {
                key: "B".into(),
                value: "${A}/${SYS}".into(),
                expand: true,
                platform: None,
            },
            EnvEntry {
                key: "C".into(),
                value: "${B}!".into(),
                expand: true,
                platform: None,
            },
        ];
        let sys = |k: &str| (k == "SYS").then(|| "sysval".to_string());
        let map = resolve_env(&entries, &sys).unwrap();
        assert_eq!(map["A"], "1");
        assert_eq!(map["B"], "1/sysval");
        // 注意：B 已在 map 中，C 引用 B 的展开值
        assert_eq!(map["C"], "1/sysval!");
    }

    #[test]
    fn expand_false_is_literal() {
        let entries = vec![EnvEntry {
            key: "LIT".into(),
            value: "${HOME}/x".into(),
            expand: false,
            platform: None,
        }];
        let map = resolve_env(&entries, &|_| None).unwrap();
        assert_eq!(map["LIT"], "${HOME}/x");
    }

    #[test]
    fn undefined_var_is_error() {
        let entries = vec![EnvEntry {
            key: "X".into(),
            value: "${NOPE}/y".into(),
            expand: true,
            platform: None,
        }];
        let errs = resolve_env(&entries, &|_| None).unwrap_err();
        assert!(errs[0].contains("NOPE"));
    }

    #[test]
    fn only_expands_dollar_brace_syntax() {
        let entries = vec![EnvEntry {
            key: "X".into(),
            value: "$HOME and ${HOME}".into(),
            expand: true,
            platform: None,
        }];
        let map = resolve_env(&entries, &|k| (k == "HOME").then(|| "/h".to_string())).unwrap();
        // $HOME（无花括号）不支持，按字面量保留
        assert_eq!(map["X"], "$HOME and /h");
    }

    #[test]
    fn unclosed_reference_is_error() {
        let entries = vec![EnvEntry {
            key: "X".into(),
            value: "${HOME".into(),
            expand: true,
            platform: None,
        }];
        assert!(resolve_env(&entries, &|_| None).is_err());
    }
}
