//! Tauri 命令层（HLD §4.1 Command）：前端 → core 的唯一通道。
//!
//! 校验失败以 `Err(Vec<String>)` 返回逐项错误（FR-2.4）；保存 = 校验 → 写文件 → 热载应用。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tauri::State;
use tauri_plugin_opener::OpenerExt;
use traysupervisor_core::config::{
    join_command_line, parse_byte_size, split_command_line, split_env_prefixed_line, system_var,
    validate_app, Autorestart, EnvEntry, LogFilePolicy, LogPolicy, MatcherConfig, PlatformDef,
    PlatformRegistry, PlatformVariant, ServiceConfig, SpawnMode,
};
use traysupervisor_core::events::LogLine;
use traysupervisor_core::expand_tilde;
use traysupervisor_core::term::TermScreen;

use crate::AppState;

/// 编辑表单入参：命令行为单行（§3.4 切分），env/matcher 与模型一致；
/// `platform` = 基础配置字段的平台变体（FR-2.13，键 ∈ 平台注册表）
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
    #[serde(default)]
    pub platform: BTreeMap<String, PlatformVariantIn>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnvEntryIn {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub expand: bool,
    /// 行级平台标签（FR-2.13）：None = 全部平台
    #[serde(default)]
    pub platform: Option<String>,
}

/// 平台变体入参（DESIGN §4.7）：None / 空串 = 跟随默认值
#[derive(Debug, Clone, Deserialize)]
pub struct PlatformVariantIn {
    #[serde(default)]
    pub command_line: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub spawn_mode: Option<String>,
    #[serde(default)]
    pub term_interactive: Option<bool>,
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
    let max_bytes = match parse_byte_size(&input.log_max_bytes) {
        Ok(v) => v,
        Err(e) => return Err(vec![format!("log_max_bytes：{e}")]),
    };
    // 平台变体（FR-2.13）：切分各平台命令行；全空变体跳过
    let mut platform: BTreeMap<String, PlatformVariant> = BTreeMap::new();
    for (p, v) in &input.platform {
        let mut pv = PlatformVariant::default();
        if let Some(line) = v
            .command_line
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            match split_command_line(line) {
                Ok((prog, pargs)) => {
                    pv.command = prog;
                    pv.args = pargs;
                }
                Err(e) => {
                    errors.push(format!("command.{p}：{e}"));
                    continue;
                }
            }
        }
        if let Some(c) = v.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            pv.directory = Some(expand_tilde(c));
        }
        if let Some(sm) = &v.spawn_mode {
            if !sm.is_empty() {
                pv.spawn_mode = Some(match sm.to_ascii_lowercase().as_str() {
                    "pipe" => SpawnMode::Pipe,
                    "pty" => SpawnMode::Pty,
                    other => {
                        errors.push(format!("spawn_mode.{p}：非法值 {other:?}（pipe / pty）"));
                        continue;
                    }
                });
            }
        }
        pv.term_interactive = v.term_interactive;
        if !pv.is_empty() {
            platform.insert(p.clone(), pv);
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(ServiceConfig {
        id: input.id.trim().to_string(),
        name: {
            let n = input.name.trim().to_string();
            if n.is_empty() {
                input.id.trim().to_string()
            } else {
                n
            }
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
            .map(|e| EnvEntry {
                key: e.key.trim().to_string(),
                value: e.value.clone(),
                expand: e.expand,
                platform: e.platform.clone(),
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
        platform,
    })
}

/// 平台变体回显（DESIGN §4.7）：命令行重新 join 为单行（roundtrip 安全）
#[derive(Debug, Clone, Serialize)]
pub struct PlatformVariantOut {
    pub command_line: String,
    pub cwd: Option<String>,
    pub spawn_mode: Option<String>,
    pub term_interactive: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceBriefOut {
    #[serde(flatten)]
    pub brief: traysupervisor_core::ServiceBrief,
    /// 默认（基础）命令行单行（join §3.4 规则），编辑表单「默认」子页使用
    pub command_line: String,
    /// 当前平台生效命令行（DESIGN §4.2）：托盘/编辑页「本机实际执行」提示用
    pub effective_command_line: String,
    /// 平台变体回显（含非本机平台，存盘无损）
    pub platform: BTreeMap<String, PlatformVariantOut>,
    /// 日志单文件上限的带单位展示值（如 `10MB`；未启用文件日志为空，FR-2.2 / D-16）
    pub log_max_bytes_text: String,
}

fn registry_of(state: &State<'_, AppState>) -> PlatformRegistry {
    PlatformRegistry::new(&state.mgr.current_config().settings)
}

fn variant_out(v: &PlatformVariant) -> PlatformVariantOut {
    PlatformVariantOut {
        command_line: join_command_line(&v.command, &v.args),
        cwd: v.directory.as_ref().map(|d| d.display().to_string()),
        spawn_mode: v.spawn_mode.map(|m| m.as_str().to_string()),
        term_interactive: v.term_interactive,
    }
}

#[tauri::command]
pub fn list_services(state: State<'_, AppState>) -> Vec<ServiceBriefOut> {
    let reg = registry_of(&state);
    state
        .mgr
        .list()
        .into_iter()
        .map(|b| {
            let command_line = join_command_line(&b.config.command, &b.config.args);
            let effective_command_line = {
                let eff = b.config.resolve_for(&reg);
                join_command_line(&eff.command, &eff.args)
            };
            let platform = b
                .config
                .platform
                .iter()
                .map(|(p, v)| (p.clone(), variant_out(v)))
                .collect();
            let log_max_bytes_text = b
                .config
                .log
                .file
                .as_ref()
                .map(|f| traysupervisor_core::config::format_byte_size(f.max_bytes))
                .unwrap_or_default();
            ServiceBriefOut {
                brief: b,
                command_line,
                effective_command_line,
                platform,
                log_max_bytes_text,
            }
        })
        .collect()
}

/// 粘贴整行解析（FR-2.12 / DESIGN §3）：前缀 KEY=VALUE → 环境变量，其余为命令行
#[derive(Debug, Clone, Serialize)]
pub struct PastedLineOut {
    pub env: Vec<EnvEntry>,
    pub command_line: String,
}

#[tauri::command]
pub fn parse_pasted_line(line: String) -> Result<PastedLineOut, String> {
    let (env, command_line) = split_env_prefixed_line(&line)?;
    Ok(PastedLineOut { env, command_line })
}

/// 平台注册表信息（DESIGN §4.7）：UI 平台下拉选项、「本机」角标与解析链提示
#[derive(Debug, Clone, Serialize)]
pub struct PlatformInfoOut {
    pub native_os: String,
    pub platforms: Vec<PlatformDef>,
    pub current: Option<String>,
    pub chain: Vec<String>,
}

#[tauri::command]
pub fn get_platform_info(state: State<'_, AppState>) -> PlatformInfoOut {
    let settings = state.mgr.current_config().settings;
    let reg = PlatformRegistry::new(&settings);
    PlatformInfoOut {
        native_os: traysupervisor_core::config::native_os().to_string(),
        platforms: reg
            .all_names()
            .into_iter()
            .map(|name| {
                let family = settings
                    .platforms
                    .iter()
                    .find(|d| d.name == name)
                    .and_then(|d| d.family.clone());
                PlatformDef { name, family }
            })
            .collect(),
        current: settings.current_platform,
        chain: reg.chain(),
    }
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
    traysupervisor_core::config::save_app_config(&cfg, mgr.config_path()).map_err(|e| vec![e])?;
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
    traysupervisor_core::config::save_app_config(&cfg, mgr.config_path()).map_err(|e| vec![e])?;
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
/// 与实际 spawn 环境一致：先按平台标签过滤合并（DESIGN §4.3），再展开；
/// 未显式配置 PATH 时展示运行时注入的兜底 PATH
#[tauri::command]
pub fn get_env_resolved_preview(
    state: State<'_, AppState>,
    input: ServiceInput,
) -> Result<BTreeMap<String, String>, String> {
    let svc = build_service_config(&input).map_err(|e| e.join("\n"))?;
    let reg = registry_of(&state);
    let eff = svc.resolve_for(&reg);
    let mut env = traysupervisor_core::config::resolve_env(&eff.env, &system_var)
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
            state
                .mgr
                .reload(cfg, traysupervisor_core::service::ReloadMode::Reread)
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
pub fn open_log_dir(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
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
