//! 服务管理集成测试（FR-3/FR-4/FR-5 验收）：真实进程 spawn/停止/自动重启/env 注入。
//! 平台：Unix/macOS CI 验证 setsid + killpg 树杀路径（Windows 分支由 spike 覆盖）。

use std::time::{Duration, Instant};

use traysupervisor_core::config::{Autorestart, EnvEntry, ServiceConfig, SpawnMode};
use traysupervisor_core::events::SvcState;
use traysupervisor_core::ServiceManager;

fn svc(id: &str, program: &str, args: &[&str]) -> ServiceConfig {
    ServiceConfig {
        id: id.to_string(),
        command: program.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        startsecs: 0,
        ..Default::default()
    }
}

fn wait_state(mgr: &ServiceManager, id: &str, want: SvcState, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let st = mgr
            .list()
            .into_iter()
            .find(|b| b.config.id == id)
            .map(|b| b.state);
        if st == Some(want) {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_pred<F: Fn() -> bool>(f: F, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    f()
}

#[test]
fn pipe_lifecycle_start_stop() {
    let cfg = traysupervisor_core::config::AppConfig {
        services: vec![svc("t-sleep", "/bin/sleep", &["30"])],
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let mgr = ServiceManager::new(cfg, dir.path().join("config.ini")).unwrap();
    mgr.start("t-sleep").unwrap();

    assert!(wait_state(&mgr, "t-sleep", SvcState::Running, Duration::from_secs(3)));
    let pid = mgr.list()[0].pid;
    assert!(pid.is_some(), "RUNNING 状态应带 PID");

    mgr.stop("t-sleep").unwrap();
    assert!(wait_state(&mgr, "t-sleep", SvcState::Stopped, Duration::from_secs(3)));
    // FR-3.3：停止后进程应已回收（kill(pid,0) 报 ESRCH）
    if let Some(pid) = pid {
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        assert!(!alive, "停止后进程 {pid} 仍存活");
    }
    mgr.shutdown_all();
}

#[test]
fn unexpected_exit_goes_exited() {
    let cfg = traysupervisor_core::config::AppConfig {
        services: vec![ServiceConfig {
            autorestart: Autorestart::No,
            ..svc("t-exit", "/bin/sleep", &["1"])
        }],
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let mgr = ServiceManager::new(cfg, dir.path().join("config.ini")).unwrap();
    mgr.start("t-exit").unwrap();
    assert!(wait_state(&mgr, "t-exit", SvcState::Exited, Duration::from_secs(5)));
    let brief = &mgr.list()[0];
    assert_eq!(brief.exit_code, Some(0));
    mgr.shutdown_all();
}

#[test]
fn autorestart_always_restarts_after_kill9() {
    let cfg = traysupervisor_core::config::AppConfig {
        services: vec![ServiceConfig {
            autorestart: Autorestart::Always,
            ..svc("t-restart", "/bin/sleep", &["30"])
        }],
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let mgr = ServiceManager::new(cfg, dir.path().join("config.ini")).unwrap();
    mgr.start("t-restart").unwrap();
    assert!(wait_state(&mgr, "t-restart", SvcState::Running, Duration::from_secs(3)));
    let pid1 = mgr.list()[0].pid.unwrap();

    // kill -9（FR-3.1 验收：autorestart=always 时 kill -9 后自动拉起）
    unsafe {
        libc::kill(pid1 as i32, libc::SIGKILL);
    }
    // 先看到非 Running（EXITED→BACKOFF 或直接 BACKOFF），随后新 PID 回到 RUNNING
    assert!(
        wait_pred(
            || {
                mgr.list()[0]
                    .pid
                    .map(|p| p != pid1)
                    .unwrap_or(false)
                    && mgr.list()[0].state == SvcState::Running
            },
            Duration::from_secs(8)
        ),
        "未在预期时间内以新 PID 重启：{:?}",
        mgr.list()[0]
    );
    mgr.shutdown_all();
}

#[test]
fn spawn_error_goes_backoff_then_fatal() {
    let cfg = traysupervisor_core::config::AppConfig {
        services: vec![ServiceConfig {
            startretries: 1,
            ..svc("t-fatal", "/nonexistent-binary-xyz", &[])
        }],
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let mgr = ServiceManager::new(cfg, dir.path().join("config.ini")).unwrap();
    mgr.start("t-fatal").unwrap();
    // 第一次失败 → BACKOFF(1s) → 重试失败 → FATAL
    assert!(wait_state(&mgr, "t-fatal", SvcState::Backoff, Duration::from_secs(2)));
    assert!(wait_state(&mgr, "t-fatal", SvcState::Fatal, Duration::from_secs(5)));
    mgr.shutdown_all();
}

#[test]
fn stdout_capture_and_env_expansion() {
    let cfg = traysupervisor_core::config::AppConfig {
        services: vec![ServiceConfig {
            env: vec![
                EnvEntry { key: "TRAY_A".into(), value: "1".into(), expand: false },
                EnvEntry {
                    key: "TRAY_B".into(),
                    value: "${TRAY_A}-x".into(),
                    expand: true,
                },
            ],
            ..svc("t-echo", "/usr/bin/env", &[])
        }],
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let mgr = ServiceManager::new(cfg, dir.path().join("config.ini")).unwrap();
    mgr.start("t-echo").unwrap();
    assert!(wait_state(&mgr, "t-echo", SvcState::Exited, Duration::from_secs(5)));
    // env 展开结果出现在子进程环境中（FR-2 验收：子进程环境正确）
    assert!(
        wait_pred(
            || {
                mgr.get_log("t-echo", 0, 2000)
                    .unwrap_or_default()
                    .iter()
                    .any(|l| l.text.contains("TRAY_B=1-x"))
            },
            Duration::from_secs(2)
        ),
        "日志中未找到 TRAY_B=1-x：{:?}",
        mgr.get_log("t-echo", 0, 2000)
    );
    mgr.shutdown_all();
}

#[test]
fn matcher_url_hit_via_real_output() {
    let cfg = traysupervisor_core::config::AppConfig {
        services: vec![svc(
            "t-url",
            "/bin/echo",
            &["Serving at http://127.0.0.1:8000"],
        )],
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let mgr = ServiceManager::new(cfg, dir.path().join("config.ini")).unwrap();
    mgr.start("t-url").unwrap();
    // FR-5 验收：输出 URL 后 1 秒量级内出现「🌐 打开 …」快捷项
    assert!(
        wait_pred(
            || {
                mgr.match_items("t-url")
                    .iter()
                    .any(|i| i.label == "🌐 打开 http://127.0.0.1:8000"
                        && i.url.as_deref() == Some("http://127.0.0.1:8000"))
            },
            Duration::from_secs(3)
        ),
        "未生成 URL 快捷项：{:?}",
        mgr.match_items("t-url")
    );
    // match_url 路由（FR-1.7 / FR-5.6）
    let hash = mgr.match_items("t-url")[0].hash;
    assert_eq!(
        mgr.match_url("t-url", hash).as_deref(),
        Some("http://127.0.0.1:8000")
    );
    // 服务重启：旧快捷项随 spawn 清空（FR-5.5，单元级已验证 clear），
    // 新一次运行的相同输出会重新命中——用系统行证明重启确实发生
    mgr.restart("t-url").unwrap();
    assert!(
        wait_pred(
            || {
                mgr.get_log("t-url", 0, 500)
                    .unwrap_or_default()
                    .iter()
                    .filter(|l| l.text.contains("[traysv] 已启动"))
                    .count()
                    >= 2
            },
            Duration::from_secs(3)
        ),
        "未观察到重启"
    );
    assert!(
        wait_pred(|| !mgr.match_items("t-url").is_empty(), Duration::from_secs(3)),
        "重启后新输出应重新生成快捷项"
    );
    mgr.shutdown_all();
}

#[test]
fn pty_mode_capture_and_term_snapshot() {
    let cfg = traysupervisor_core::config::AppConfig {
        services: vec![ServiceConfig {
            spawn_mode: SpawnMode::Pty,
            ..svc("t-pty", "/bin/echo", &["pty-hello"])
        }],
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let mgr = ServiceManager::new(cfg, dir.path().join("config.ini")).unwrap();
    mgr.start("t-pty").unwrap();
    assert!(wait_state(&mgr, "t-pty", SvcState::Exited, Duration::from_secs(5)));
    assert!(
        wait_pred(
            || {
                mgr.get_log("t-pty", 0, 200)
                    .unwrap_or_default()
                    .iter()
                    .any(|l| l.text.contains("pty-hello"))
            },
            Duration::from_secs(3)
        ),
        "PTY 输出未进入日志"
    );
    let screen = mgr.term_snapshot("t-pty").unwrap();
    let text: String = screen
        .lines
        .iter()
        .flat_map(|row| row.iter().map(|r| r.t.clone()))
        .collect();
    assert!(text.contains("pty-hello"), "终端快照未包含输出：{text:?}");
    mgr.shutdown_all();
}

#[test]
fn hot_reload_applies_diff() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.ini");
    std::fs::write(&path, "[program:hr-a]\ncommand=/bin/sleep\n").unwrap();

    let cfg = traysupervisor_core::config::load_app_config(&path).unwrap();
    let mgr = ServiceManager::new(cfg, path.clone()).unwrap();
    assert!(wait_state(&mgr, "hr-a", SvcState::Stopped, Duration::from_secs(2)));

    // 手写新配置：改名 + 新增服务（added）+ autostart 不自动启动（CONFIG-SPEC §8）
    std::fs::write(
        &path,
        "[program:hr-a]\ncommand=/bin/sleep\nname=改名\n\n[program:hr-b]\ncommand=/bin/echo\n",
    )
    .unwrap();
    // 主动触发（watcher 线程也会触发，此处直接调用验证语义）
    mgr.reload_from_file().unwrap();
    let ids: Vec<String> = mgr.list().into_iter().map(|b| b.config.id).collect();
    assert_eq!(ids, vec!["hr-a", "hr-b"]);
    let a = &mgr.list()[0];
    assert_eq!(a.config.name, "改名");
    assert_eq!(a.state, SvcState::Stopped, "热载 added/未受影响服务不自动启动");
    mgr.shutdown_all();
}
