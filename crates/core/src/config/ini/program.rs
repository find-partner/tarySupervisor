//! [program:<id>] 节解析：字段映射、日志策略、matcher.N 编号键（CONFIG-SPEC §5）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::parse::{parse_bool, parse_u32, parse_u64, prop_get, Props};
use crate::config::{
    Autorestart, LogFilePolicy, LogPolicy, MatcherConfig, ServiceConfig, SpawnMode,
};
use crate::config::parse_environment;
use crate::expand_tilde;

pub(super) fn parse_program(
    id: &str,
    props: Props<'_>,
    base_dir: Option<&Path>,
    section_name: &str,
) -> Result<ServiceConfig, String> {
    let mut svc = ServiceConfig {
        id: id.to_string(),
        ..Default::default()
    };
    let get = |key: &str| prop_get(props, key);

    // command：单行 → 程序 + 参数数组（§3.4）
    if let Some(v) = get("command") {
        let (prog, args) = crate::config::split_command_line(v)
            .map_err(|e| format!("[{section_name}] command：{e}"))?;
        svc.command = prog;
        svc.args = args;
    }

    if let Some(v) = get("name") {
        svc.name = v.to_string();
    }
    if let Some(v) = get("directory") {
        let t = v.trim();
        if !t.is_empty() {
            svc.cwd = Some(expand_tilde(t));
        }
    }
    if let Some(v) = get("autostart") {
        svc.autostart = parse_bool(v, &format!("[{section_name}] autostart"))?;
    }
    if let Some(v) = get("autorestart") {
        svc.autorestart = match v.trim() {
            "no" => Autorestart::No,
            "on-failure" => Autorestart::OnFailure,
            "always" => Autorestart::Always,
            other => {
                return Err(format!(
                    "[{section_name}] autorestart：非法值 {other:?}（no / on-failure / always）"
                ))
            }
        };
    }
    if let Some(v) = get("startsecs") {
        svc.startsecs = parse_u64(v, &format!("[{section_name}] startsecs"))?;
    }
    if let Some(v) = get("startretries") {
        svc.startretries = parse_u32(v, &format!("[{section_name}] startretries"))?;
    }
    if let Some(v) = get("stopwaitsecs") {
        svc.stop_timeout_secs = parse_u64(v, &format!("[{section_name}] stopwaitsecs"))?;
    }
    if let Some(v) = get("spawn_mode") {
        svc.spawn_mode = match v.trim() {
            "pipe" => SpawnMode::Pipe,
            "pty" => SpawnMode::Pty,
            other => return Err(format!("[{section_name}] spawn_mode：非法值 {other:?}（pipe / pty）")),
        };
    }
    if let Some(v) = get("shell") {
        svc.shell = parse_bool(v, &format!("[{section_name}] shell"))?;
    }
    if let Some(v) = get("redirect_stderr") {
        svc.redirect_stderr = parse_bool(v, &format!("[{section_name}] redirect_stderr"))?;
    }
    if let Some(v) = get("term_interactive") {
        svc.term_interactive = parse_bool(v, &format!("[{section_name}] term_interactive"))?;
    }
    if let Some(v) = get("environment") {
        svc.env = parse_environment(v).map_err(|e| format!("[{section_name}] environment：{e}"))?;
    }

    parse_log_policy(props, base_dir, section_name, &mut svc)?;
    svc.matchers = parse_matchers(props, section_name)?;

    Ok(svc)
}

/// 日志文件策略（stdout_logfile / maxbytes / backups / compress）
fn parse_log_policy(
    props: Props<'_>,
    base_dir: Option<&Path>,
    section_name: &str,
    svc: &mut ServiceConfig,
) -> Result<(), String> {
    let get = |key: &str| prop_get(props, key);
    let logfile = get("stdout_logfile").map(str::trim).filter(|s| !s.is_empty());
    if let Some(lf) = logfile {
        let path = resolve_log_path(lf, base_dir);
        let max_bytes = match get("stdout_logfile_maxbytes") {
            // 带单位大小字符串（10MB / 512KB / 1G…）或纯字节数（FR-2.2 / D-16）
            Some(v) => crate::config::parse_byte_size(v)
                .map_err(|e| format!("[{section_name}] stdout_logfile_maxbytes：{e}"))?,
            None => 10_485_760,
        };
        let max_files = match get("stdout_logfile_backups") {
            Some(v) => parse_u32(v, &format!("[{section_name}] stdout_logfile_backups"))?,
            None => 3,
        };
        let compress = match get("logfile_compress") {
            Some(v) => parse_bool(v, &format!("[{section_name}] logfile_compress"))?,
            None => false,
        };
        svc.log = LogPolicy {
            file: Some(LogFilePolicy { path, max_bytes, max_files, compress }),
        };
    } else if get("stdout_logfile_maxbytes").is_some()
        || get("stdout_logfile_backups").is_some()
        || get("logfile_compress").is_some()
    {
        return Err(format!(
            "[{section_name}] 配置了 stdout_logfile_maxbytes/backups/logfile_compress，但 stdout_logfile 为空"
        ));
    }
    Ok(())
}

/// matcher.N 编号键（§5）：编号从 1 连续递增；每个必须有 pattern。
fn parse_matchers(props: Props<'_>, section_name: &str) -> Result<Vec<MatcherConfig>, String> {
    // 未知键检测（CONFIG-SPEC §1/§7.7）：先收集 matcher.*，其余必须命中已知键
    const KNOWN_KEYS: &[&str] = &[
        "name", "command", "directory", "autostart", "autorestart", "startsecs", "startretries",
        "stopwaitsecs", "environment", "redirect_stderr", "stdout_logfile", "term_interactive",
        "stdout_logfile_maxbytes", "stdout_logfile_backups", "spawn_mode", "logfile_compress",
        "shell",
    ];
    let mut matcher_raw: BTreeMap<u32, (Option<String>, Option<String>, Option<u32>)> =
        BTreeMap::new();
    for (k, v) in props.iter() {
        if let Some(rest) = k.strip_prefix("matcher.") {
            let Some((num, field)) = rest.split_once('.') else {
                return Err(format!("[{section_name}] 非法的 matcher 键 {k}（应为 matcher.N.pattern 等）"));
            };
            let n: u32 = num
                .parse()
                .map_err(|_| format!("[{section_name}] matcher 编号 {num:?} 非法"))?;
            let entry = matcher_raw.entry(n).or_default();
            match field {
                "pattern" => entry.0 = Some(v.to_string()),
                "template" => entry.1 = Some(v.to_string()),
                "max_items" => {
                    entry.2 = Some(parse_u32(v, &format!("[{section_name}] matcher.{n}.max_items"))?)
                }
                other => return Err(format!("[{section_name}] matcher.{n}：未知字段 {other}")),
            }
            continue;
        }
        if !KNOWN_KEYS.contains(&k.as_str()) {
            return Err(format!("[{section_name}] 未知键 {k}"));
        }
    }

    let mut matchers = Vec::with_capacity(matcher_raw.len());
    for (idx, (n, fields)) in matcher_raw.into_iter().enumerate() {
        let expect = (idx + 1) as u32;
        if n != expect {
            return Err(format!(
                "[{section_name}] matcher 编号必须从 1 连续递增：期望 {expect}，实际 {n}"
            ));
        }
        let (pattern, template, max_items) = fields;
        let Some(pattern) = pattern else {
            return Err(format!("[{section_name}] matcher.{n} 缺少 pattern"));
        };
        matchers.push(MatcherConfig {
            pattern,
            template: template.unwrap_or_else(|| "🌐 打开 {0}".to_string()),
            max_items: max_items.unwrap_or(5),
        });
    }
    Ok(matchers)
}

/// stdout_logfile 路径解析：`~` 前缀展开；相对路径相对配置文件所在目录（§3.1）。
fn resolve_log_path(p: &str, base_dir: Option<&Path>) -> PathBuf {
    let expanded = expand_tilde(p);
    if expanded.is_absolute() {
        expanded
    } else {
        match base_dir {
            Some(dir) => dir.join(expanded),
            None => expanded,
        }
    }
}
