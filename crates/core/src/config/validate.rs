//! 配置校验（CONFIG-SPEC §7 / SRS FR-2.4）：加载与保存均执行，错误逐项输出。
//!
//! 平台维度（FR-2.13 / DESIGN §4.4）：
//! - 当前平台解析链：完整校验（env 引用可解析——生效表 + 本机系统环境）；
//! - 其他平台的环境变量条目：仅结构性校验（键名、NUL、同标签重复键），
//!   `${VAR}` 引用不做本机校验——未在本表内解析的引用可能是目标平台系统变量，
//!   无法与笔误区分，一律放行，由目标平台加载/启动时的完整校验兜底；
//! - spawn_mode / term_interactive 组合校验按"默认 + 每个配置了变体的平台"展开。

use super::{expand_env_value, resolve_env, AppConfig, PlatformRegistry, ServiceConfig, SpawnMode};
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
    if cfg.settings.log_lines < LOG_LINES_MIN || cfg.settings.log_lines > LOG_LINES_MAX {
        errors.push(format!(
            "[traysupervisor] log_lines：超出范围 {}–{}",
            LOG_LINES_MIN, LOG_LINES_MAX
        ));
    }
    errors.extend(validate_platform_settings(&cfg.settings));
    let reg = PlatformRegistry::new(&cfg.settings);
    let mut seen = HashSet::new();
    for svc in &cfg.services {
        if !seen.insert(svc.id.as_str()) {
            errors.push(format!("[program:{}] id 重复", svc.id));
        }
        errors.extend(validate_service(svc, &reg));
    }
    errors
}

/// 全局节平台声明校验（DESIGN §4.4）：名字 slug、不与内置/其他声明冲突、
/// 家族枚举（解析期已查）、current_platform 必须是注册表成员。
fn validate_platform_settings(settings: &super::GlobalSettings) -> Vec<String> {
    let mut errors = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for def in &settings.platforms {
        let tag = "[traysupervisor] platforms";
        if def.name.is_empty()
            || !def
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            errors.push(format!(
                "{tag} 平台名 {:?} 非法（仅小写字母、数字、'-'、'_'）",
                def.name
            ));
            continue;
        }
        if super::BUILTIN_PLATFORMS.contains(&def.name.as_str()) {
            errors.push(format!("{tag} 平台名 {} 与内置平台冲突", def.name));
            continue;
        }
        if !seen.insert(def.name.as_str()) {
            errors.push(format!("{tag} 平台名 {} 重复声明", def.name));
            continue;
        }
        if let Some(f) = &def.family {
            if !super::BUILTIN_PLATFORMS.contains(&f.as_str()) {
                errors.push(format!(
                    "{tag} 平台 {name} 的家族 {f:?} 非法（须为 {}）",
                    super::BUILTIN_PLATFORMS.join(" / "),
                    name = def.name,
                    f = f,
                ));
            }
        }
    }
    if let Some(cur) = &settings.current_platform {
        let known = super::BUILTIN_PLATFORMS.contains(&cur.as_str())
            || settings.platforms.iter().any(|d| &d.name == cur);
        if !known {
            errors.push(format!(
                "[traysupervisor] current_platform {cur:?} 未注册（须为内置平台或 platforms 中声明的自定义平台）"
            ));
        }
    }
    errors
}

/// 校验单个服务配置（保存时执行 FR-2.4：命令可切分、正则可编译、env 可解析等）。
pub fn validate_service(svc: &ServiceConfig, reg: &PlatformRegistry) -> Vec<String> {
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
            errors.push(format!(
                "{tag} stdout_logfile_maxbytes 最大 {}（支持 KB/MB/GB 单位）",
                super::format_byte_size(MAX_BYTES_MAX)
            ));
        }
        if f.max_files > BACKUPS_MAX {
            errors.push(format!(
                "{tag} stdout_logfile_backups 必须 0–{}",
                BACKUPS_MAX
            ));
        }
    }

    // 平台变体（FR-2.13 / DESIGN §4.4）：后缀注册表成员、命令行 NUL、组合校验
    for (platform, v) in &svc.platform {
        if !reg.is_known(platform) {
            errors.push(format!(
                "{tag} 平台变体后缀 {platform:?} 未注册（须为 {}）",
                reg.all_names().join(" / ")
            ));
        }
        if !v.command.is_empty() {
            if v.command.contains('\0') {
                errors.push(format!("{tag} command.{platform} 含非法字符 NUL"));
            }
            for (i, a) in v.args.iter().enumerate() {
                if a.contains('\0') {
                    errors.push(format!("{tag} command.{platform} args[{i}] 含非法字符 NUL"));
                }
            }
        }
        let sm = v.spawn_mode.unwrap_or(svc.spawn_mode);
        let ti = v.term_interactive.unwrap_or(svc.term_interactive);
        if ti && sm != SpawnMode::Pty {
            errors.push(format!(
                "{tag} 平台 {platform}：term_interactive 需要 spawn_mode=pty（管道模式不支持输入）"
            ));
        }
    }
    if svc.term_interactive && svc.spawn_mode != SpawnMode::Pty {
        errors.push(format!(
            "{tag} term_interactive 需要 spawn_mode=pty（管道模式不支持输入）"
        ));
    }

    // 环境变量：键合法、同标签内无重复、当前平台生效表完整展开（FR-2.3 / FR-2.4）
    let mut keys: HashSet<(String, Option<String>)> = HashSet::new();
    for e in &svc.env {
        if e.key.is_empty() {
            errors.push(format!("{tag} environment 存在空键名"));
            continue;
        }
        if e.key
            .chars()
            .any(|c| c.is_whitespace() || c == '=' || c == '"' || c == ',')
        {
            errors.push(format!("{tag} environment 键 {} 含非法字符", e.key));
        }
        if let Some(p) = &e.platform {
            if !reg.is_known(p) {
                errors.push(format!(
                    "{tag} environment 键 {} 的平台标签 {p:?} 未注册（须为 {} 或 all）",
                    e.key,
                    reg.all_names().join(" / ")
                ));
            }
        }
        if !keys.insert((e.key.clone(), e.platform.clone())) {
            let scope = e
                .platform
                .as_deref()
                .map(|p| format!("@{p}"))
                .unwrap_or_default();
            errors.push(format!("{tag} environment 键 {} {scope}重复", e.key));
        }
        if e.value.contains('\0') {
            errors.push(format!("{tag} environment 键 {} 的值含 NUL", e.key));
        }
    }
    // 当前平台生效表：完整校验（含系统环境引用），与实际 spawn 环境一致
    if let Err(errs) = resolve_env(&svc.effective_env(&reg.chain()), &system_var) {
        for e in errs {
            errors.push(format!("{tag} environment：{e}"));
        }
    }
    // 其他平台的带标签条目：引用可解析性本机无法核实（可能是目标平台系统变量），放行；
    // 仅语法级错误在此处早发现（未闭合 ${} 与空展开值，resolve 语义之外的静态检查）
    let cur = reg.chain();
    for e in &svc.env {
        let Some(p) = &e.platform else { continue };
        if cur.iter().any(|c| c == p) {
            continue;
        }
        if !e.expand {
            continue;
        }
        if e.value.is_empty() {
            errors.push(format!(
                "{tag} environment 键 {}（@{p}）：开启替换的值为空",
                e.key
            ));
            continue;
        }
        if let Err(err) = expand_env_value(&e.value, &|_| None) {
            // 未定义引用（最常见）不算错；仅报语法问题
            if !err.contains("未定义的环境变量") {
                errors.push(format!("{tag} environment 键 {}（@{p}）：{err}", e.key));
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        EnvEntry, GlobalSettings, MatcherConfig, PlatformDef, PlatformVariant, ServiceConfig,
    };

    fn svc(id: &str, cmd: &str) -> ServiceConfig {
        ServiceConfig {
            id: id.to_string(),
            command: cmd.to_string(),
            ..Default::default()
        }
    }

    fn reg() -> PlatformRegistry {
        PlatformRegistry::new(&GlobalSettings::default())
    }

    #[test]
    fn valid_service_passes() {
        let s = svc("dev-docs", "python");
        assert!(validate_service(&s, &reg()).is_empty());
    }

    #[test]
    fn id_slug_enforced() {
        let s = svc("Bad Id!", "python");
        let errs = validate_service(&s, &reg());
        assert!(errs.iter().any(|e| e.contains("id 仅允许")), "{errs:?}");
    }

    #[test]
    fn empty_command_rejected() {
        let s = svc("a", "  ");
        let errs = validate_service(&s, &reg());
        assert!(errs.iter().any(|e| e.contains("command 不能为空")));
    }

    #[test]
    fn bad_regex_rejected() {
        let s = ServiceConfig {
            matchers: vec![MatcherConfig {
                pattern: "(unclosed".into(),
                ..Default::default()
            }],
            ..svc("a", "x")
        };
        let errs = validate_service(&s, &reg());
        assert!(errs.iter().any(|e| e.contains("正则无法编译")));
    }

    #[test]
    fn undefined_env_var_rejected() {
        let s = ServiceConfig {
            env: vec![EnvEntry {
                key: "X".into(),
                value: "${DEFINITELY_UNDEFINED_VAR_42}".into(),
                expand: true,
                platform: None,
            }],
            ..svc("a", "x")
        };
        let errs = validate_service(&s, &reg());
        assert!(errs
            .iter()
            .any(|e| e.contains("DEFINITELY_UNDEFINED_VAR_42")));
    }

    #[test]
    fn tagged_env_for_other_platform_not_blocked_by_system_refs() {
        // 在非 Windows 平台上，@windows 条目引用本机不存在的系统变量应放行（DESIGN §4.4）
        let s = ServiceConfig {
            env: vec![EnvEntry {
                key: "X".into(),
                value: "${DEFINITELY_UNDEFINED_VAR_42}".into(),
                expand: true,
                platform: Some("windows".into()),
            }],
            ..svc("a", "x")
        };
        let errs = validate_service(&s, &reg());
        if super::super::native_os() == "windows" {
            assert!(errs
                .iter()
                .any(|e| e.contains("DEFINITELY_UNDEFINED_VAR_42")));
        } else {
            assert!(errs.is_empty(), "{errs:?}");
        }
    }

    #[test]
    fn unclosed_ref_syntax_caught_even_for_other_platform() {
        let s = ServiceConfig {
            env: vec![EnvEntry {
                key: "X".into(),
                value: "${HOME".into(),
                expand: true,
                platform: Some("windows".into()),
            }],
            ..svc("a", "x")
        };
        let errs = validate_service(&s, &reg());
        assert!(errs.iter().any(|e| e.contains("闭合")), "{errs:?}");
    }

    #[test]
    fn combo_validation_per_platform() {
        let mut s = svc("a", "x");
        s.spawn_mode = super::SpawnMode::Pty;
        s.term_interactive = true;
        s.platform.insert(
            "windows".into(),
            PlatformVariant {
                spawn_mode: Some(super::SpawnMode::Pipe),
                term_interactive: Some(false),
                ..Default::default()
            },
        );
        assert!(validate_service(&s, &reg()).is_empty(), "成对覆盖应通过");

        let mut bad = svc("a", "x");
        bad.term_interactive = true;
        bad.platform.insert(
            "windows".into(),
            PlatformVariant {
                spawn_mode: Some(super::SpawnMode::Pipe),
                ..Default::default()
            },
        );
        let errs = validate_service(&bad, &reg());
        assert!(errs.iter().any(|e| e.contains("平台 windows")), "{errs:?}");
    }

    #[test]
    fn unknown_platform_suffix_rejected() {
        let mut s = svc("a", "x");
        s.platform.insert(
            "nope".into(),
            PlatformVariant {
                spawn_mode: Some(super::SpawnMode::Pty),
                ..Default::default()
            },
        );
        let errs = validate_service(&s, &reg());
        assert!(errs.iter().any(|e| e.contains("未注册")), "{errs:?}");
    }

    #[test]
    fn tagged_duplicate_key_rejected_only_within_same_tag() {
        let mut ok = svc("a", "x");
        ok.env = vec![
            EnvEntry {
                key: "K".into(),
                value: "1".into(),
                expand: false,
                platform: None,
            },
            EnvEntry {
                key: "K".into(),
                value: "2".into(),
                expand: false,
                platform: Some("windows".into()),
            },
        ];
        assert!(validate_service(&ok, &reg()).is_empty());

        let mut bad = svc("a", "x");
        bad.env = vec![
            EnvEntry {
                key: "K".into(),
                value: "1".into(),
                expand: false,
                platform: None,
            },
            EnvEntry {
                key: "K".into(),
                value: "2".into(),
                expand: false,
                platform: None,
            },
        ];
        assert!(validate_service(&bad, &reg())
            .iter()
            .any(|e| e.contains("重复")));
    }

    #[test]
    fn platform_settings_validation() {
        let mut s = GlobalSettings {
            platforms: vec![
                PlatformDef {
                    name: "home-mac".into(),
                    family: Some("macos".into()),
                },
                PlatformDef {
                    name: "macos".into(),
                    family: None,
                }, // 与内置冲突
            ],
            current_platform: Some("home-mac".into()),
            ..Default::default()
        };
        let errs = validate_platform_settings(&s);
        assert!(
            errs.iter().any(|e| e.contains("与内置平台冲突")),
            "{errs:?}"
        );

        s = GlobalSettings {
            current_platform: Some("ghost".into()),
            ..s
        };
        let errs = validate_platform_settings(&s);
        assert!(
            errs.iter().any(|e| e.contains("current_platform")),
            "{errs:?}"
        );
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
}
