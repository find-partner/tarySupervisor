//! INI 编解码（CONFIG-SPEC）：手写解析器 → 语义模型；手写序列化保证转义可控。
//!
//! 解析语义（刻意最小化，不借助 rust-ini 以保证值原样保留）：
//! - 值不做任何转义/去引号处理，`\` 与 `"` 原样保留（CONFIG-SPEC §3.4 第 3 条的必然要求）；
//! - `#` / `;` 行注释；节与键按文件顺序保留；
//! - 节：`[traysupervisor]`（可选）与 `[program:<id>]`；未知节/未知键 = 校验错误（§1/§7.1）；
//! - matcher 以 `matcher.N.pattern/template/max_items` 编号键表达，N 从 1 连续递增（§5）；
//! - 保存：先写临时文件再原子改名；Unix 权限 0600（NFR-5）。

use super::{
    join_command_line, parse_environment, serialize_environment, split_command_line, validate_app,
    Autorestart, AppConfig, GlobalSettings, LogFilePolicy, LogPolicy, MatcherConfig, ServiceConfig,
    SpawnMode,
};
use crate::expand_tilde;
use std::path::{Path, PathBuf};

pub fn load_app_config(path: &Path) -> Result<AppConfig, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取配置失败：{e}"))?;
    parse_ini_str(&text, path.parent())
}

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

type Props<'a> = &'a [(String, String)];

fn prop_get<'a>(props: Props<'a>, key: &str) -> Option<&'a str> {
    // 同键多次出现时取最后一次（INI 惯例）
    props
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn parse_program(
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
        let (prog, args) = split_command_line(v)
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
    if let Some(v) = get("redirect_stderr") {
        svc.redirect_stderr = parse_bool(v, &format!("[{section_name}] redirect_stderr"))?;
    }
    if let Some(v) = get("environment") {
        svc.env = parse_environment(v).map_err(|e| format!("[{section_name}] environment：{e}"))?;
    }

    // 日志文件策略
    let logfile = get("stdout_logfile").map(str::trim).filter(|s| !s.is_empty());
    if let Some(lf) = logfile {
        let path = resolve_log_path(lf, base_dir);
        let max_bytes = match get("stdout_logfile_maxbytes") {
            Some(v) => parse_u64(v, &format!("[{section_name}] stdout_logfile_maxbytes"))?,
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

    // 未知键检测（CONFIG-SPEC §1/§7.7）：先收集 matcher.*，其余必须命中已知键
    const KNOWN_KEYS: &[&str] = &[
        "name", "command", "directory", "autostart", "autorestart", "startsecs", "startretries",
        "stopwaitsecs", "environment", "redirect_stderr", "stdout_logfile",
        "stdout_logfile_maxbytes", "stdout_logfile_backups", "spawn_mode", "logfile_compress",
    ];
    let mut matcher_raw: std::collections::BTreeMap<u32, (Option<String>, Option<String>, Option<u32>)> =
        std::collections::BTreeMap::new();
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

    // matcher.N：编号从 1 连续递增（§5）；每个必须有 pattern
    let n_matchers = matcher_raw.len();
    let mut matchers: Vec<MatcherConfig> = Vec::with_capacity(n_matchers);
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
    svc.matchers = matchers;

    Ok(svc)
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

fn parse_bool(v: &str, key: &str) -> Result<bool, String> {
    match v.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("{key}：非法布尔值 {other:?}（true / false）")),
    }
}

fn parse_u32(v: &str, key: &str) -> Result<u32, String> {
    v.trim()
        .parse::<u32>()
        .map_err(|_| format!("{key}：非法整数 {v:?}"))
}

fn parse_u64(v: &str, key: &str) -> Result<u64, String> {
    v.trim()
        .parse::<u64>()
        .map_err(|_| format!("{key}：非法整数 {v:?}"))
}

/// 序列化为 INI 文本（CONFIG-SPEC §6 示例风格）。
pub fn to_ini_string(cfg: &AppConfig) -> String {
    let mut out = String::new();
    out.push_str("# TraySupervisor 配置（supervisord 风格）；完整规范见 docs/CONFIG-SPEC.md\n");
    out.push_str("\n[traysupervisor]\n");
    out.push_str(&format!("log_lines={}\n", cfg.settings.log_lines));
    out.push_str(&format!("notify_on_failure={}\n", cfg.settings.notify_on_failure));
    for svc in &cfg.services {
        out.push_str(&format!("\n[program:{}]\n", svc.id));
        if !svc.name.is_empty() && svc.name != svc.id {
            out.push_str(&format!("name={}\n", svc.name));
        }
        out.push_str(&format!("command={}\n", join_command_line(&svc.command, &svc.args)));
        if let Some(dir) = &svc.cwd {
            out.push_str(&format!("directory={}\n", dir.display()));
        }
        out.push_str(&format!("autostart={}\n", svc.autostart));
        out.push_str(&format!("autorestart={}\n", svc.autorestart.as_str()));
        out.push_str(&format!("startsecs={}\n", svc.startsecs));
        out.push_str(&format!("startretries={}\n", svc.startretries));
        out.push_str(&format!("stopwaitsecs={}\n", svc.stop_timeout_secs));
        if svc.spawn_mode != SpawnMode::Pipe {
            out.push_str(&format!("spawn_mode={}\n", svc.spawn_mode.as_str()));
        }
        if svc.redirect_stderr {
            out.push_str("redirect_stderr=true\n");
        }
        if !svc.env.is_empty() {
            out.push_str(&format!("environment={}\n", serialize_environment(&svc.env)));
        }
        if let Some(f) = &svc.log.file {
            out.push_str(&format!("stdout_logfile={}\n", f.path.display()));
            out.push_str(&format!("stdout_logfile_maxbytes={}\n", f.max_bytes));
            out.push_str(&format!("stdout_logfile_backups={}\n", f.max_files));
            if f.compress {
                out.push_str("logfile_compress=true\n");
            }
        }
        for (i, m) in svc.matchers.iter().enumerate() {
            out.push_str(&format!("matcher.{}.pattern={}\n", i + 1, m.pattern));
            out.push_str(&format!("matcher.{}.template={}\n", i + 1, m.template));
            out.push_str(&format!("matcher.{}.max_items={}\n", i + 1, m.max_items));
        }
    }
    out
}

/// 原子保存：临时文件 + 改名；Unix 权限 0600（NFR-5）。
pub fn save_app_config(cfg: &AppConfig, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let text = to_ini_string(cfg);
    let tmp = path.with_extension("ini.tmp");
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| format!("写入配置失败：{e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("保存配置失败：{e}"))?;
    Ok(())
}

/// 生成默认（空服务）配置并写盘；返回内容。
pub fn ensure_default_config(path: &Path) -> Result<AppConfig, String> {
    if path.exists() {
        return load_app_config(path);
    }
    let cfg = AppConfig::default();
    save_app_config(&cfg, path)?;
    Ok(cfg)
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{diff_configs, EnvEntry};

    const SAMPLE: &str = r#"
# 注释
[traysupervisor]
log_lines=2000
notify_on_failure=true

[program:dev-docs]
name=文档站
command=python -m http.server 8000 --directory "C:\work\docs"
directory=C:\work\docs
autostart=true
autorestart=on-failure
startsecs=3
stopwaitsecs=10
stdout_logfile=~/.traysupervisor/logs/dev-docs.log
stdout_logfile_maxbytes=10485760
stdout_logfile_backups=3
logfile_compress=true
matcher.1.pattern=https?://\S+
matcher.1.template=🌐 打开 {0}
matcher.1.max_items=5

[program:api-server]
command=node --max-old-space-size=512 main.js
directory=~/work/api
spawn_mode=pipe
autostart=false
autorestart=always
environment=NODE_ENV="development", LOG_DIR="!${HOME}/logs/api", PORT="3000"
redirect_stderr=false
"#;

    #[test]
    fn parses_spec_sample() {
        let cfg = parse_ini_str(SAMPLE, None).unwrap();
        assert_eq!(cfg.settings.log_lines, 2000);
        assert_eq!(cfg.services.len(), 2);
        let d = &cfg.services[0];
        assert_eq!(d.id, "dev-docs");
        assert_eq!(d.name, "文档站");
        assert_eq!(d.command, "python");
        assert_eq!(d.args, vec!["-m", "http.server", "8000", "--directory", r"C:\work\docs"]);
        assert_eq!(d.autorestart, Autorestart::OnFailure);
        assert_eq!(d.matchers.len(), 1);
        assert_eq!(d.matchers[0].pattern, r"https?://\S+");
        let a = &cfg.services[1];
        assert_eq!(a.autorestart, Autorestart::Always);
        assert_eq!(a.env.len(), 3);
        assert_eq!(a.env[1].expand, true);
        assert_eq!(a.env[1].value, "${HOME}/logs/api");
    }

    #[test]
    fn unknown_key_rejected() {
        let s = "[program:a]\ncommand=x\nbogus_key=1\n";
        let err = parse_ini_str(s, None).unwrap_err();
        assert!(err.contains("未知键 bogus_key"), "{err}");
    }

    #[test]
    fn unknown_section_rejected() {
        let s = "[webapp]\nfoo=1\n";
        assert!(parse_ini_str(s, None).is_err());
    }

    #[test]
    fn matcher_numbering_gap_rejected() {
        let s = "[program:a]\ncommand=x\nmatcher.1.pattern=a\nmatcher.3.pattern=b\n";
        let err = parse_ini_str(s, None).unwrap_err();
        assert!(err.contains("连续递增"), "{err}");
    }

    #[test]
    fn utf8_bom_tolerated() {
        let s = "\u{feff}[program:a]\ncommand=x\n";
        assert!(parse_ini_str(s, None).is_ok(), "带 BOM 的配置应可解析");
    }

    #[test]
    fn bool_enum_rejected() {
        let s = "[program:a]\ncommand=x\nautostart=yes\n";
        let err = parse_ini_str(s, None).unwrap_err();
        assert!(err.contains("autostart"), "{err}");
        let s2 = "[program:a]\ncommand=x\nautorestart=sometimes\n";
        assert!(parse_ini_str(s2, None).unwrap_err().contains("autorestart"));
    }

    #[test]
    fn duplicate_id_rejected() {
        let s = "[program:a]\ncommand=x\n\n[program:a]\ncommand=y\n";
        let err = parse_ini_str(s, None).unwrap_err();
        assert!(err.contains("program:a"), "{err}");
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.ini");
        let cfg = AppConfig {
            settings: GlobalSettings { log_lines: 500, notify_on_failure: false },
            services: vec![
                ServiceConfig {
                    id: "web".into(),
                    name: "网页服务".into(),
                    command: r"C:\Program Files\python.exe".into(),
                    args: vec!["-m".into(), "http.server".into(), "--bind".into(), "127.0.0.1".into()],
                    cwd: None,
                    env: vec![
                        EnvEntry { key: "A".into(), value: "x y".into(), expand: false },
                        EnvEntry { key: "B".into(), value: "${A}/z".into(), expand: true },
                    ],
                    autostart: true,
                    autorestart: Autorestart::Always,
                    startsecs: 2,
                    startretries: 3,
                    stop_timeout_secs: 5,
                    spawn_mode: SpawnMode::Pty,
                    log: LogPolicy {
                        file: Some(LogFilePolicy {
                            path: PathBuf::from("/tmp/web.log"),
                            max_bytes: 1024,
                            max_files: 2,
                            compress: true,
                        }),
                    },
                    matchers: vec![MatcherConfig {
                        pattern: r"port (\d+)".into(),
                        template: "端口 {0}".into(),
                        max_items: 3,
                    }],
                    ..Default::default()
                },
                ServiceConfig { id: "b".into(), command: "echo".into(), ..Default::default() },
            ],
        };
        save_app_config(&cfg, &path).unwrap();
        let loaded = load_app_config(&path).unwrap();
        assert_eq!(loaded, cfg);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "NFR-5：配置文件权限 0600");
        }
    }

    #[test]
    fn relative_logfile_resolved_against_base_dir() {
        let dir = tempfile::tempdir().unwrap();
        let s = "[program:a]\ncommand=x\nstdout_logfile=logs/a.log\n";
        let cfg = parse_ini_str(s, Some(dir.path())).unwrap();
        let f = cfg.services[0].log.file.as_ref().unwrap();
        assert_eq!(f.path, dir.path().join("logs/a.log"));
    }

    #[test]
    fn diff_detects_hard_and_soft_changes() {
        let old = vec![ServiceConfig { id: "a".into(), command: "x".into(), ..Default::default() }];
        let mut new = old.clone();
        new[0].name = "改名".into();
        let d = diff_configs(&old, &new);
        assert!(d.changed.is_empty() && d.soft_changed == vec!["a".to_string()]);
        new[0].command = "y".into();
        let d = diff_configs(&old, &new);
        assert_eq!(d.changed, vec!["a".to_string()]);
    }
}
