//! 复现「改环境变量 → 重启服务 → 仍是旧 env」的用户报告（GUI 保存路径 + watcher 路径）。

use std::time::{Duration, Instant};

use traysupervisor_core::config::{AppConfig, EnvEntry, ServiceConfig};
use traysupervisor_core::events::SvcState;
use traysupervisor_core::ServiceManager;

fn svc_with_env(value: &str) -> ServiceConfig {
    ServiceConfig {
        id: "t-env".into(),
        command: "/bin/sh".into(),
        args: vec!["-c".into(), "echo MYVAR=[$MYVAR]; sleep 30".into()],
        startsecs: 0,
        env: vec![EnvEntry {
            key: "MYVAR".into(),
            value: value.into(),
            expand: false,
            platform: None,
        }],
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

/// 轮询日志直到出现包含 want 的行（排除上次会话已出现的行：按出现次数判断）
fn wait_log_contains(mgr: &ServiceManager, id: &str, want: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let hit = mgr
            .get_log(id, 0, 500)
            .unwrap_or_default()
            .iter()
            .any(|l| l.text.contains(want));
        if hit {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn dump_log(mgr: &ServiceManager, id: &str) -> String {
    mgr.get_log(id, 0, 500)
        .unwrap_or_default()
        .iter()
        .map(|l| l.text.clone())
        .collect::<Vec<_>>()
        .join(" | ")
}

#[test]
fn gui_save_then_restart_uses_new_env() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.ini");

    let cfg = AppConfig {
        services: vec![svc_with_env("old")],
        ..Default::default()
    };
    traysupervisor_core::config::save_app_config(&cfg, &path).unwrap();
    let mgr = ServiceManager::new(cfg, path.clone()).unwrap();
    mgr.start("t-env").unwrap();
    assert!(wait_state(
        &mgr,
        "t-env",
        SvcState::Running,
        Duration::from_secs(5)
    ));
    assert!(wait_log_contains(
        &mgr,
        "t-env",
        "MYVAR=[old]",
        Duration::from_secs(5)
    ));

    // GUI 保存语义：先写文件，再 apply_saved_config（不自动重启）
    let new_cfg = AppConfig {
        services: vec![svc_with_env("fixed")],
        ..Default::default()
    };
    traysupervisor_core::config::save_app_config(&new_cfg, &path).unwrap();
    let delta = mgr.apply_saved_config().expect("apply_saved_config 应成功");
    println!("delta = {delta:?}");
    assert_eq!(
        delta.changed,
        vec!["t-env".to_string()],
        "env 变更应识别为 hard change"
    );

    mgr.restart("t-env").unwrap();
    assert!(
        wait_log_contains(&mgr, "t-env", "MYVAR=[fixed]", Duration::from_secs(5)),
        "重启后应使用新 env；日志：{}",
        dump_log(&mgr, "t-env")
    );
    mgr.shutdown_all();
}

#[test]
fn external_edit_watcher_hot_reload_then_restart_uses_new_env() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.ini");

    let cfg = AppConfig {
        services: vec![svc_with_env("old")],
        ..Default::default()
    };
    traysupervisor_core::config::save_app_config(&cfg, &path).unwrap();
    let mgr = ServiceManager::new(cfg, path.clone()).unwrap();
    mgr.start("t-env").unwrap();
    assert!(wait_state(
        &mgr,
        "t-env",
        SvcState::Running,
        Duration::from_secs(5)
    ));
    assert!(wait_log_contains(
        &mgr,
        "t-env",
        "MYVAR=[old]",
        Duration::from_secs(5)
    ));

    // 模拟外部编辑：只改文件，等 watcher 去抖 500ms + FSEvents 延迟
    let new_cfg = AppConfig {
        services: vec![svc_with_env("fixed")],
        ..Default::default()
    };
    traysupervisor_core::config::save_app_config(&new_cfg, &path).unwrap();
    std::thread::sleep(Duration::from_secs(3));

    // watcher（reload_from_file，restart_changed=true）应对运行中服务自动重启
    assert!(
        wait_log_contains(&mgr, "t-env", "MYVAR=[fixed]", Duration::from_secs(5)),
        "热加载应自动以新 env 重启；日志：{}；配置回显：{:?}",
        dump_log(&mgr, "t-env"),
        mgr.list()[0].config.env
    );
    mgr.shutdown_all();
}
