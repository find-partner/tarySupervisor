//! Tauri 命令层（HLD §4.1 Command）：前端 → core 的唯一通道。
//!
//! 校验失败以 `Err(Vec<String>)` 返回逐项错误（FR-2.4）；保存 = 校验 → 写文件 → 热载应用。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tauri::State;
use tauri_plugin_opener::OpenerExt;
use traysupervisor_core::config::{
    join_command_line, split_command_line, system_var, validate_app, Autorestart, LogFilePolicy,
    LogPolicy, MatcherConfig, ServiceConfig, SpawnMode,
};
use traysupervisor_core::events::LogLine;
use traysupervisor_core::expand_tilde;
use traysupervisor_core::term::TermScreen;

use crate::AppState;

/// 编辑表单入参：命令行为单行（§3.4 切分），env/matcher 与模型一致
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceInput {
    pub id: String,
    pub name: String,
    pub command_line: String,
    pub cwd: Option<String>,
    pub env: Vec<EnvEntryIn>,
    pub redirect_stderr: bool,
    pub autostart: bool,
    pub autorestart: String,
    pub startsecs: u64,
    pub startretries: u32,
    pub stopwaitsecs: u64,
    pub spawn_mode: String,
    /// 经用户登录 shell 启动（继承 .zshrc/.bashrc 环境，CONFIG-SPEC §3.5）
    #[serde(default)]
    pub shell: bool,
    /// 终端视图可写（交互，仅 Pty 有效，SRS FR-6.4）
    #[serde(default)]
    pub term_interactive: bool,
    pub log_file_enabled: bool,
    pub log_path: Option<String>,
    /// 带单位大小（`10MB` / `512KB` / `1GB` 或纯字节，FR-2.2 / D-16）
    pub log_max_bytes: String,
    pub log_max_files: u32,
    pub log_compress: bool,
    pub matchers: Vec<MatcherIn>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnvEntryIn {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub expand: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MatcherIn {
    pub pattern: String,
    #[serde(default)]
    pub template: String,
    #[serde(default = "d5")]
    pub max_items: u32,
}

fn d5() -> u32 {
    5
}

fn parse_autorestart(s: &str) -> Autorestart {
    match s.to_ascii_lowercase().as_str() {
        "on-failure" | "onfailure" => Autorestart::OnFailure,
        "always" => Autorestart::Always,
        _ => Autorestart::No,
    }
}

fn parse_spawn_mode(s: &str) -> SpawnMode {
    match s.to_ascii_lowercase().as_str() {
        "pty" => SpawnMode::Pty,
        _ => SpawnMode::Pipe,
    }
}

fn build_service_config(input: &ServiceInput) -> Result<ServiceConfig, Vec<String>> {
    let mut errors = Vec::new();
    let (command, args) = match split_command_line(&input.command_line) {
        Ok(v) => v,
        Err(e) => {
            errors.push(format!("command：{e}"));
            ("__invalid__".to_string(), vec![])
        }
    };
    if !errors.is_empty() {
        return Err(errors);
    }
    // 日志单文件上限：带单位大小 → 字节（FR-2.2 / HLD D-16）
    let max_bytes = match traysupervisor_core::config::parse_byte_size(&input.log_max_bytes) {
        Ok(v) => v,
        Err(e) => return Err(vec![format!("log_max_bytes：{e}")]),
    };
    Ok(ServiceConfig {
        id: input.id.trim().to_string(),
        name: {
            let n = input.name.trim().to_string();
            if n.is_empty() { input.id.trim().to_string() } else { n }
        },
        command,
        args,
        cwd: input
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(expand_tilde),
        env: input
            .env
            .iter()
            .map(|e| traysupervisor_core::config::EnvEntry {
                key: e.key.trim().to_string(),
                value: e.value.clone(),
                expand: e.expand,
            })
            .collect(),
        redirect_stderr: input.redirect_stderr,
        autostart: input.autostart,
        autorestart: parse_autorestart(&input.autorestart),
        startsecs: input.startsecs,
        startretries: input.startretries,
        stop_timeout_secs: input.stopwaitsecs.max(1),
        spawn_mode: parse_spawn_mode(&input.spawn_mode),
        shell: input.shell,
        term_interactive: input.term_interactive,
        log: LogPolicy {
            file: if input.log_file_enabled {
                // 路径留空 → 以内置默认路径生成（SRS FR-2.2），不再静默降级为不启用；
                // 「stdout_logfile 空 = 仅内存」仅属 INI 语义，GUI 勾选即启用
                let path = input
                    .log_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(expand_tilde)
                    .unwrap_or_else(|| traysupervisor_core::default_log_path(input.id.trim()));
                Some(LogFilePolicy {
                    path,
                    max_bytes,
                    max_files: input.log_max_files,
                    compress: input.log_compress,
                })
            } else {
                None
            },
        },
        matchers: input
            .matchers
            .iter()
            .map(|m| MatcherConfig {
                pattern: m.pattern.clone(),
                template: if m.template.is_empty() {
                    "🌐 打开 {0}".to_string()
                } else {
                    m.template.clone()
                },
                max_items: m.max_items,
            })
            .collect(),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceBriefOut {
    #[serde(flatten)]
    pub brief: traysupervisor_core::ServiceBrief,
    /// 单行命令行（join §3.4 规则），编辑表单直接使用
    pub command_line: String,
    /// 日志单文件上限的带单位展示值（如 `10MB`；未启用文件日志为空，FR-2.2 / D-16）
    pub log_max_bytes_text: String,
}

#[tauri::command]
pub fn list_services(state: State<'_, AppState>) -> Vec<ServiceBriefOut> {
    state
        .mgr
        .list()
        .into_iter()
        .map(|b| {
            let command_line = join_command_line(&b.config.command, &b.config.args);
            let log_max_bytes_text = b
                .config
                .log
                .file
                .as_ref()
                .map(|f| traysupervisor_core::config::format_byte_size(f.max_bytes))
                .unwrap_or_default();
            ServiceBriefOut { brief: b, command_line, log_max_bytes_text }
        })
        .collect()
}

#[tauri::command]
pub fn upsert_service(state: State<'_, AppState>, input: ServiceInput) -> Result<(), Vec<String>> {
    let mgr = &state.mgr;
    let svc = build_service_config(&input)?;
    let mut cfg = mgr.current_config();

    // 替换同 id（保持顺序）或追加
    match cfg.services.iter_mut().find(|s| s.id == svc.id) {
        Some(slot) => *slot = svc,
        None => cfg.services.push(svc),
    }

    // 保存前整体校验（FR-2.4），任一错误阻止保存
    let errors = validate_app(&cfg);
    if !errors.is_empty() {
        return Err(errors);
    }

    // 写文件 → GUI 保存语义（SRS FR-2.10）：应用配置但不自动重启运行中服务，
    // 重启由前端"需重启生效"提示显式触发（HLD D-10）
    traysupervisor_core::config::save_app_config(&cfg, mgr.config_path())
        .map_err(|e| vec![e])?;
    mgr.apply_saved_config().map(|_| ())
}

#[tauri::command]
pub fn delete_service(state: State<'_, AppState>, id: String) -> Result<(), Vec<String>> {
    let mgr = &state.mgr;
    let mut cfg = mgr.current_config();
    let before = cfg.services.len();
    cfg.services.retain(|s| s.id != id);
    if cfg.services.len() == before {
        return Err(vec![format!("服务 {id} 不存在")]);
    }
    traysupervisor_core::config::save_app_config(&cfg, mgr.config_path())
        .map_err(|e| vec![e])?;
    mgr.reload_from_file().map(|_| ())
}

#[tauri::command]
pub fn start_service(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.mgr.start(&id)
}

#[tauri::command]
pub fn stop_service(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.mgr.stop(&id)
}

#[tauri::command]
pub fn restart_service(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.mgr.restart(&id)
}

#[tauri::command]
pub fn get_log(state: State<'_, AppState>, id: String, since: u64, limit: usize) -> Vec<LogLine> {
    state.mgr.get_log(&id, since, limit).unwrap_or_default()
}

/// Matcher 快捷项跳转（FR-5.6：仅 http/https）
#[tauri::command]
pub fn open_match_url(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    svc_id: String,
    hash: u64,
) -> Result<(), String> {
    let url = state
        .mgr
        .match_url(&svc_id, hash)
        .ok_or_else(|| format!("快捷项不存在或已被清空（{svc_id}#{hash}）"))?;
    if !traysupervisor_core::matcher::is_safe_url(&url) {
        return Err(format!("已拒绝非 http/https 链接：{url}"));
    }
    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| format!("打开浏览器失败：{e}"))
}

/// env 展开预览（编辑页「预览」按钮）；
/// 与实际 spawn 环境一致：未显式配置 PATH 时展示运行时注入的兜底 PATH
#[tauri::command]
pub fn get_env_resolved_preview(
    input: ServiceInput,
) -> Result<BTreeMap<String, String>, String> {
    let svc = build_service_config(&input).map_err(|e| e.join("\n"))?;
    let mut env = traysupervisor_core::config::resolve_env(&svc.env, &system_var)
        .map_err(|errs| errs.join("\n"))?;
    traysupervisor_core::service::shellenv::inject_fallback_path(&mut env);
    Ok(env)
}

#[tauri::command]
pub fn reload_config(
    state: State<'_, AppState>,
    mode: String,
) -> Result<traysupervisor_core::config::ConfigDelta, Vec<String>> {
    let m = if mode.eq_ignore_ascii_case("reread") {
        traysupervisor_core::service::ReloadMode::Reread
    } else {
        traysupervisor_core::service::ReloadMode::Update
    };
    match m {
        traysupervisor_core::service::ReloadMode::Reread => {
            let cfg = traysupervisor_core::config::load_app_config(state.mgr.config_path())
                .map_err(|e| vec![e])?;
            state.mgr.reload(cfg, traysupervisor_core::service::ReloadMode::Reread)
        }
        traysupervisor_core::service::ReloadMode::Update => state.mgr.reload_from_file(),
    }
}

#[tauri::command]
pub fn term_snapshot(state: State<'_, AppState>, id: String) -> Option<TermScreen> {
    state.mgr.term_snapshot(&id)
}

/// 交互终端输入转发（SRS FR-6.4）：键盘/粘贴字节 → PTY master
#[tauri::command]
pub fn write_stdin(state: State<'_, AppState>, id: String, data: Vec<u8>) -> Result<(), String> {
    state.mgr.write_stdin(&id, data)
}

#[tauri::command]
pub fn open_log_dir(app: tauri::AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    // 查找服务的日志文件路径（未配置则提示）
    let log_path = state
        .mgr
        .list()
        .into_iter()
        .find(|b| b.config.id == id)
        .and_then(|b| b.config.log.file.map(|f| f.path))
        .ok_or_else(|| "该服务未配置日志文件".to_string())?;
    let dir = log_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or(log_path);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建日志目录失败：{e}"))?;
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| format!("打开目录失败：{e}"))
}

/// 打开（或聚焦）服务日志窗口（编辑页操作栏 FR-2.11 与托盘菜单共用入口）
#[tauri::command]
pub fn open_log_window(app: tauri::AppHandle, id: String) {
    crate::wins::open_log(&app, &id);
}

/// 打开（或聚焦）服务终端视图窗口（编辑页操作栏 FR-2.11 与托盘菜单共用入口）
#[tauri::command]
pub fn open_term_window(app: tauri::AppHandle, id: String) {
    crate::wins::open_term(&app, &id);
}
