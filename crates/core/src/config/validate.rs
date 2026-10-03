//! 配置校验（CONFIG-SPEC §7 / SRS FR-2.4）：加载与保存均执行，错误逐项输出。

use super::{
    resolve_env, AppConfig, ServiceConfig, BUILTIN_URL_PATTERN, BUILTIN_URL_TEMPLATE,
};
use regex::Regex;
use std::collections::HashSet;

// 数值上限（CONFIG-SPEC §7.3）
pub const LOG_LINES_MIN: u32 = 100;
pub const LOG_LINES_MAX: u32 = 100000;
pub const START_RETRIES_MAX: u32 = 10;
pub const BACKUPS_MAX: u32 = 100;
pub const MAX_BYTES_MAX: u64 = 1024 * 1024 * 1024; // 1GB
pub const MATCHERS_MAX: usize = 20;
pub const MATCHER_MAX_ITEMS_MAX: u32 = 10;

/// 系统环境变量查询（测试可注入）；实现为闭包以隔离 std::env。
pub fn system_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// 校验整个配置，返回逐项错误（中文、带定位）；空 = 通过。
pub fn validate_app(cfg: &AppConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if cfg.settings.log_lines < LOG_LINES_MIN
        || cfg.settings.log_lines > LOG_LINES_MAX
    {
        errors.push(format!(
            "[traysupervisor] log_lines：超出范围 {}–{}",
            LOG_LINES_MIN, LOG_LINES_MAX
        ));
    }
    let mut seen = HashSet::new();
    for svc in &cfg.services {
        if !seen.insert(svc.id.as_str()) {
            errors.push(format!("[program:{}] id 重复", svc.id));
        }
        errors.extend(validate_service(svc));
    }
    errors
}

/// 校验单个服务配置（保存时执行 FR-2.4：命令可切分、正则可编译、env 可解析等）。
pub fn validate_service(svc: &ServiceConfig) -> Vec<String> {
    let tag = format!("[program:{}]", svc.id);
    let mut errors = Vec::new();

    // id slug：仅 [a-z0-9-_]（CONFIG-SPEC §3）
    if svc.id.is_empty() {
        errors.push(format!("{tag} id 不能为空"));
    } else if !svc
        .id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        errors.push(format!("{tag} id 仅允许小写字母、数字、'-'、'_'"));
    }

    // command 非空（模型中已切分为程序+参数）
    if svc.command.trim().is_empty() {
        errors.push(format!("{tag} command 不能为空"));
    }
    for (i, a) in svc.args.iter().enumerate() {
        if a.contains('\0') {
            errors.push(format!("{tag} args[{i}] 含非法字符 NUL"));
        }
    }

    // 数值范围（CONFIG-SPEC §7.3）
    if svc.stop_timeout_secs < 1 {
        errors.push(format!("{tag} stopwaitsecs 必须 ≥ 1"));
    }
    if svc.startretries > START_RETRIES_MAX {
        errors.push(format!("{tag} startretries 必须 0–{}", START_RETRIES_MAX));
    }
    if let Some(f) = &svc.log.file {
        if f.max_bytes > MAX_BYTES_MAX {
            errors.push(format!("{tag} stdout_logfile_maxbytes 最大 {} 字节", MAX_BYTES_MAX));
        }
        if f.max_files > BACKUPS_MAX {
            errors.push(format!("{tag} stdout_logfile_backups 必须 0–{}", BACKUPS_MAX));
        }
    }

    // env：键合法、无重复、展开可解析（FR-2.3 / FR-2.4）
    let mut keys = HashSet::new();
    for e in &svc.env {
        if e.key.is_empty() {
            errors.push(format!("{tag} environment 存在空键名"));
            continue;
        }
        if e.key.chars().any(|c| c.is_whitespace() || c == '=' || c == '"' || c == ',') {
            errors.push(format!("{tag} environment 键 {} 含非法字符", e.key));
        }
        if !keys.insert(e.key.clone()) {
            errors.push(format!("{tag} environment 键 {} 重复", e.key));
        }
        if e.value.contains('\0') {
            errors.push(format!("{tag} environment 键 {} 的值含 NUL", e.key));
        }
    }
    if let Err(errs) = resolve_env(&svc.env, &system_var) {
        for e in errs {
            errors.push(format!("{tag} environment：{e}"));
        }
    }

    // Matcher：正则可编译、数量与上限（FR-2.4 / FR-5.4 / CONFIG-SPEC §5）
    if svc.matchers.len() > MATCHERS_MAX {
        errors.push(format!("{tag} matcher 数量超过 {}", MATCHERS_MAX));
    }
    for (i, m) in svc.matchers.iter().enumerate() {
        if let Err(e) = Regex::new(&m.pattern) {
            errors.push(format!("{tag} matcher.{} 正则无法编译：{}", i + 1, e));
        }
        if m.max_items < 1 || m.max_items > MATCHER_MAX_ITEMS_MAX {
            errors.push(format!(
                "{tag} matcher.{} max_items 必须 1–{}",
                i + 1,
                MATCHER_MAX_ITEMS_MAX
            ));
        }
        if m.template.is_empty() {
            errors.push(format!("{tag} matcher.{} template 不能为空", i + 1));
        }
    }

    errors
}

/// Matcher 空时启用内置 HTTP URL 规则（FR-5.3：无需配置即可出现「🌐 打开 …」）。
pub fn effective_matchers(svc: &ServiceConfig) -> Vec<super::MatcherConfig> {
    if svc.matchers.is_empty() {
        vec![super::MatcherConfig {
            pattern: BUILTIN_URL_PATTERN.to_string(),
            template: BUILTIN_URL_TEMPLATE.to_string(),
            max_items: 5,
        }]
    } else {
        svc.matchers.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EnvEntry, MatcherConfig, ServiceConfig};

    fn svc(id: &str, cmd: &str) -> ServiceConfig {
        ServiceConfig {
            id: id.to_string(),
            command: cmd.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn valid_service_passes() {
        let s = svc("dev-docs", "python");
        assert!(validate_service(&s).is_empty());
    }

    #[test]
    fn id_slug_enforced() {
        let s = svc("Bad Id!", "python");
        let errs = validate_service(&s);
        assert!(errs.iter().any(|e| e.contains("id 仅允许")), "{errs:?}");
    }

    #[test]
    fn empty_command_rejected() {
        let s = svc("a", "  ");
        let errs = validate_service(&s);
        assert!(errs.iter().any(|e| e.contains("command 不能为空")));
    }

    #[test]
    fn bad_regex_rejected() {
        let s = ServiceConfig {
            matchers: vec![MatcherConfig { pattern: "(unclosed".into(), ..Default::default() }],
            ..svc("a", "x")
        };
        let errs = validate_service(&s);
        assert!(errs.iter().any(|e| e.contains("正则无法编译")));
    }

    #[test]
    fn undefined_env_var_rejected() {
        let s = ServiceConfig {
            env: vec![EnvEntry {
                key: "X".into(),
                value: "${DEFINITELY_UNDEFINED_VAR_42}".into(),
                expand: true,
            }],
            ..svc("a", "x")
        };
        let errs = validate_service(&s);
        assert!(errs.iter().any(|e| e.contains("DEFINITELY_UNDEFINED_VAR_42")));
    }

    #[test]
    fn duplicate_id_rejected() {
        let cfg = AppConfig {
            services: vec![svc("dup", "a"), svc("dup", "b")],
            ..Default::default()
        };
        let errs = validate_app(&cfg);
        assert!(errs.iter().any(|e| e.contains("id 重复")));
    }

    #[test]
    fn builtin_url_matcher_applied_when_empty() {
        let s = svc("a", "x");
        let ms = effective_matchers(&s);
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].pattern, BUILTIN_URL_PATTERN);
    }
}
