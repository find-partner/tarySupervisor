//! INI 文本 → 有序节结构 → AppConfig 的骨架解析（program 节细节见 program.rs）。

use std::path::Path;

use super::program::parse_program;
use crate::config::{validate_app, AppConfig, GlobalSettings, PlatformDef, ServiceConfig};

/// 手写 INI 解析：产出（节名, 有序键值对）列表；值完全原样。
fn parse_ini_sections(text: &str) -> Result<Vec<(String, Vec<(String, String)>)>, String> {
    // 容忍 UTF-8 BOM（Windows 记事本保存的文件常带 BOM）
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut sections: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let Some(name) = rest.strip_suffix(']') else {
                return Err(format!("第 {lineno} 行：节名未闭合 ']'"));
            };
            if sections.iter().any(|(n, _)| n == name) {
                return Err(format!("第 {lineno} 行：节 [{name}] 重复"));
            }
            sections.push((name.to_string(), Vec::new()));
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(format!("第 {lineno} 行：缺少 '='（{line:?}）"));
        };
        let k = k.trim();
        let v = v.trim();
        if k.is_empty() {
            return Err(format!("第 {lineno} 行：空键名"));
        }
        match sections.last_mut() {
            Some((_, kvs)) => kvs.push((k.to_string(), v.to_string())),
            None => {
                return Err(format!(
                "第 {lineno} 行：键 {k} 不在任何节内（应放入 [program:<id>] 或 [traysupervisor]）"
            ))
            }
        }
    }
    Ok(sections)
}

/// 解析 INI 文本为 AppConfig（含全部校验，任一错误即整体拒绝）。
pub fn parse_ini_str(text: &str, base_dir: Option<&Path>) -> Result<AppConfig, String> {
    let sections = parse_ini_sections(text)?;
    let mut settings = GlobalSettings::default();
    let mut services: Vec<ServiceConfig> = Vec::new();
    let mut seen_global = false;

    for (name, props) in &sections {
        if name == "traysupervisor" {
            if seen_global {
                return Err("[traysupervisor] 节重复".to_string());
            }
            seen_global = true;
            for (k, v) in props {
                match k.as_str() {
                    "log_lines" => {
                        settings.log_lines = parse_u32(v, "[traysupervisor] log_lines")?;
                    }
                    "notify_on_failure" => {
                        settings.notify_on_failure =
                            parse_bool(v, "[traysupervisor] notify_on_failure")?;
                    }
                    "platforms" => {
                        settings.platforms = parse_platforms(v)?;
                    }
                    "current_platform" => {
                        let t = v.trim();
                        settings.current_platform = (!t.is_empty()).then(|| t.to_string());
                    }
                    other => return Err(format!("[traysupervisor] 未知键 {other}")),
                }
            }
        } else if let Some(id) = name.strip_prefix("program:") {
            let svc = parse_program(id, props, base_dir, name)?;
            services.push(svc);
        } else {
            return Err(format!(
                "未知节 [{name}]（仅允许 [traysupervisor] 与 [program:<id>]）"
            ));
        }
    }

    let cfg = AppConfig { settings, services };
    let errors = validate_app(&cfg);
    if !errors.is_empty() {
        return Err(format!(
            "配置校验失败（{} 项）：\n  - {}",
            errors.len(),
            errors.join("\n  - ")
        ));
    }
    Ok(cfg)
}

pub(super) type Props<'a> = &'a [(String, String)];

pub(super) fn prop_get<'a>(props: Props<'a>, key: &str) -> Option<&'a str> {
    // 同键多次出现时取最后一次（INI 惯例）
    props
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

pub(super) fn parse_bool(v: &str, key: &str) -> Result<bool, String> {
    match v.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("{key}：非法布尔值 {other:?}（true / false）")),
    }
}

pub(super) fn parse_u32(v: &str, key: &str) -> Result<u32, String> {
    v.trim()
        .parse::<u32>()
        .map_err(|_| format!("{key}：非法整数 {v:?}"))
}

pub(super) fn parse_u64(v: &str, key: &str) -> Result<u64, String> {
    v.trim()
        .parse::<u64>()
        .map_err(|_| format!("{key}：非法整数 {v:?}"))
}

/// 自定义平台列表解析（CONFIG-SPEC §2 / DESIGN §4.1）：
/// `platforms=home-mac:macos, office-win:windows, build-box`——逗号分隔，
/// 每项为 `名字[:家族]`；家族须为内置平台名（合法性在此校验，成员关系由 validate 复核）。
fn parse_platforms(v: &str) -> Result<Vec<PlatformDef>, String> {
    let mut defs = Vec::new();
    for item in v.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let (name, family) = match item.split_once(':') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (item, None),
        };
        if name.is_empty() {
            return Err(format!("[traysupervisor] platforms：{item:?} 缺少平台名"));
        }
        let family = family.filter(|f| !f.is_empty());
        if let Some(f) = family.as_deref() {
            if !crate::config::BUILTIN_PLATFORMS.contains(&f) {
                return Err(format!(
                    "[traysupervisor] platforms：平台 {name} 的家族 {f:?} 非法（须为 {}）",
                    crate::config::BUILTIN_PLATFORMS.join(" / ")
                ));
            }
        }
        defs.push(PlatformDef {
            name: name.to_string(),
            family: family.map(str::to_string),
        });
    }
    Ok(defs)
}
