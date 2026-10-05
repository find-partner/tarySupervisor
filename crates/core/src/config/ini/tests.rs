//! ini 编解码测试：样例解析、未知键/节拒绝、roundtrip、原子保存权限。

use std::path::PathBuf;

use super::{load_app_config, parse_ini_str, save_app_config, to_ini_string};
use crate::config::{
    diff_configs, Autorestart, AppConfig, EnvEntry, GlobalSettings, LogFilePolicy, LogPolicy,
    MatcherConfig, ServiceConfig, SpawnMode,
};

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
environment=HOME="/home/dev", NODE_ENV="development", LOG_DIR="!${HOME}/logs/api", PORT="3000"
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
    // 自包含样例：HOME 在表内定义（CI/Windows 无系统 HOME），LOG_DIR 引用其前面的条目
    assert_eq!(a.env.len(), 4);
    assert_eq!(a.env[0].key, "HOME");
    assert_eq!(a.env[0].value, "/home/dev");
    assert_eq!(a.env[2].expand, true);
    assert_eq!(a.env[2].value, "${HOME}/logs/api");
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
fn shell_field_roundtrip() {
    let s = "[program:a]\ncommand=x\nshell=true\n";
    let cfg = parse_ini_str(s, None).unwrap();
    assert!(cfg.services[0].shell);
    let cfg2 = parse_ini_str(&to_ini_string(&cfg), None).unwrap();
    assert_eq!(cfg2, cfg);
    // 非法值与未知键路径
    assert!(parse_ini_str("[program:a]\ncommand=x\nshell=maybe\n", None)
        .unwrap_err()
        .contains("shell"));
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
fn log_maxbytes_accepts_units_and_writes_readable() {
    // 单位写法（大小写不敏感 / 可省略 B）与纯字节数兼容（FR-2.2 / D-16）
    for (text, expect) in [
        ("10MB", 10_485_760u64),
        ("512kb", 512 * 1024),
        ("1G", 1024 * 1024 * 1024),
        ("10485760", 10_485_760),
    ] {
        let s = format!("[program:a]\ncommand=x\nstdout_logfile=/tmp/a.log\nstdout_logfile_maxbytes={text}\n");
        let cfg = parse_ini_str(&s, None).unwrap();
        assert_eq!(
            cfg.services[0].log.file.as_ref().unwrap().max_bytes,
            expect,
            "解析 {text}"
        );
    }
    // 非法单位拒绝
    let bad = "[program:a]\ncommand=x\nstdout_logfile=/tmp/a.log\nstdout_logfile_maxbytes=10TB\n";
    assert!(parse_ini_str(bad, None).unwrap_err().contains("stdout_logfile_maxbytes"));

    // 回写为可读单位且可再解析（roundtrip）
    let cfg = AppConfig {
        services: vec![ServiceConfig {
            id: "a".into(),
            command: "x".into(),
            log: LogPolicy {
                file: Some(LogFilePolicy {
                    path: PathBuf::from("/tmp/a.log"),
                    max_bytes: 10_485_760,
                    max_files: 3,
                    compress: false,
                }),
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let text = to_ini_string(&cfg);
    assert!(text.contains("stdout_logfile_maxbytes=10MB"), "{text}");
    assert_eq!(parse_ini_str(&text, None).unwrap(), cfg);
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
