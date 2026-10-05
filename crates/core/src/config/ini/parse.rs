//! INI 文本 → 有序节结构 → AppConfig 的骨架解析（program 节细节见 program.rs）。

use std::path::Path;

use crate::config::{validate_app, AppConfig, GlobalSettings, ServiceConfig};
use super::program::parse_program;

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
                return Err(format!("第 {lineno} 行：键 {k} 不在任何节内（应放入 [program:<id>] 或 [traysupervisor]）"))
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
                        settings.notify_on_failure = parse_bool(v, "[traysupervisor] notify_on_failure")?;
                    }
                    other => return Err(format!("[traysupervisor] 未知键 {other}")),
                }
            }
        } else if let Some(id) = name.strip_prefix("program:") {
            let svc = parse_program(id, props, base_dir, name)?;
            services.push(svc);
        } else {
            return Err(format!("未知节 [{name}]（仅允许 [traysupervisor] 与 [program:<id>]）"));
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
