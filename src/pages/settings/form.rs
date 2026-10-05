//! 表单状态模型（FR-2.8）：与 ServiceBriefOut / ServiceInput 的双向转换，
//! 以及"每页独立保存"的合并规则。

use leptos::*;

use crate::dto::{EnvEntry, MatcherConfig, ServiceBriefOut, ServiceInput};

#[derive(Clone, Debug, Default)]
pub struct FormState {
    pub id: String,
    pub name: String,
    pub command_line: String,
    pub cwd: String,
    pub env: Vec<EnvEntry>,
    pub redirect_stderr: bool,
    pub autostart: bool,
    pub autorestart: String,
    pub startsecs: String,
    pub startretries: String,
    pub stopwaitsecs: String,
    pub spawn_mode: String,
    pub shell: bool,
    pub term_interactive: bool,
    pub log_enabled: bool,
    pub log_path: String,
    /// 带单位大小展示值（`10MB` / `512KB` / `1GB` 或纯字节；解析在后端，FR-2.2 / D-16）
    pub log_max_bytes: String,
    pub log_max_files: String,
    pub log_compress: bool,
    pub matchers: Vec<MatcherConfig>,
}

pub fn form_from_brief(b: &ServiceBriefOut) -> FormState {
    FormState {
        id: b.config.id.clone(),
        name: b.config.name.clone(),
        command_line: b.command_line.clone(),
        cwd: b.config.cwd.clone().unwrap_or_default(),
        env: b.config.env.clone(),
        redirect_stderr: b.config.redirect_stderr,
        autostart: b.config.autostart,
        autorestart: b.config.autorestart.clone(),
        startsecs: b.config.startsecs.to_string(),
        startretries: b.config.startretries.to_string(),
        stopwaitsecs: b.config.stop_timeout_secs.to_string(),
        spawn_mode: b.config.spawn_mode.clone(),
        shell: b.config.shell,
        term_interactive: b.config.term_interactive,
        log_enabled: b.config.log.file.is_some(),
        log_path: b
            .config
            .log
            .file
            .as_ref()
            .map(|f| f.path.clone())
            .unwrap_or_default(),
        // 带单位展示值由后端格式化（list_services 的 log_max_bytes_text，FR-2.2 / D-16）
        log_max_bytes: b
            .config
            .log
            .file
            .as_ref()
            .map(|_| {
                if b.log_max_bytes_text.is_empty() {
                    "10MB".into()
                } else {
                    b.log_max_bytes_text.clone()
                }
            })
            .unwrap_or_else(|| "10MB".into()),
        log_max_files: b
            .config
            .log
            .file
            .as_ref()
            .map(|f| f.max_files.to_string())
            .unwrap_or_else(|| "3".into()),
        log_compress: b.config.log.file.as_ref().map(|f| f.compress).unwrap_or(false),
        matchers: b.config.matchers.clone(),
    }
}

pub fn input_from_form(f: &FormState) -> ServiceInput {
    ServiceInput {
        id: f.id.trim().to_string(),
        name: f.name.trim().to_string(),
        command_line: f.command_line.clone(),
        cwd: {
            let c = f.cwd.trim().to_string();
            (!c.is_empty()).then_some(c)
        },
        env: f.env.clone(),
        redirect_stderr: f.redirect_stderr,
        autostart: f.autostart,
        autorestart: f.autorestart.clone(),
        startsecs: f.startsecs.trim().parse().unwrap_or(3),
        startretries: f.startretries.trim().parse().unwrap_or(5),
        stopwaitsecs: f.stopwaitsecs.trim().parse().unwrap_or(10),
        spawn_mode: f.spawn_mode.clone(),
        shell: f.shell,
        term_interactive: f.term_interactive,
        // 勾选启用但路径留空 → 仍按启用提交，后端以内置默认路径生成（SRS FR-2.2）；
        // 旧逻辑会把空路径静默降级为「不启用」
        log_file_enabled: f.log_enabled,
        log_path: (!f.log_path.trim().is_empty()).then(|| f.log_path.trim().to_string()),
        // 带单位原样提交（后端 parse_byte_size 解析；非法值被校验拦截，FR-2.2 / D-16）
        log_max_bytes: f.log_max_bytes.clone(),
        log_max_files: f.log_max_files.trim().parse().unwrap_or(3),
        log_compress: f.log_compress,
        matchers: f.matchers.clone(),
    }
}

pub fn blank_form() -> FormState {
    FormState {
        id: String::new(),
        name: String::new(),
        command_line: String::new(),
        cwd: String::new(),
        env: vec![],
        redirect_stderr: false,
        autostart: false,
        autorestart: "No".into(),
        startsecs: "3".into(),
        startretries: "5".into(),
        stopwaitsecs: "10".into(),
        spawn_mode: "Pipe".into(),
        shell: false,
        term_interactive: false,
        log_enabled: false,
        log_path: String::new(),
        log_max_bytes: "10MB".into(),
        log_max_files: "3".into(),
        log_compress: false,
        // 内置 URL 规则随新建显式落盘（FR-5.3）；core 未向前端导出常量，
        // 与 crates/core BUILTIN_URL_PATTERN / CONFIG-SPEC §5 需同步
        matchers: vec![MatcherConfig {
            pattern: r"https?://[^\s]+".into(),
            template: "🌐 打开 {0}".into(),
            max_items: 5,
        }],
    }
}

/// 编辑模式的"每页独立保存"（FR-2.8）：以加载时快照 `loaded` 为基底，
/// 仅叠入 `tab` 页对应字段，避免把其他页的未保存变更一并提交。
/// 向导模式（新建）不走此合并——最后一步整体保存。
pub fn merged_for_save(loaded: &FormState, current: &FormState, tab: usize) -> FormState {
    let mut m = loaded.clone();
    match tab {
        0 => {
            m.id = current.id.clone();
            m.name = current.name.clone();
            m.command_line = current.command_line.clone();
            m.cwd = current.cwd.clone();
            m.spawn_mode = current.spawn_mode.clone();
            m.shell = current.shell;
            m.term_interactive = current.term_interactive;
            m.autostart = current.autostart;
            m.autorestart = current.autorestart.clone();
            m.startsecs = current.startsecs.clone();
            m.startretries = current.startretries.clone();
            m.stopwaitsecs = current.stopwaitsecs.clone();
            m.redirect_stderr = current.redirect_stderr;
        }
        1 => m.env = current.env.clone(),
        2 => {
            m.log_enabled = current.log_enabled;
            m.log_path = current.log_path.clone();
            m.log_max_bytes = current.log_max_bytes.clone();
            m.log_max_files = current.log_max_files.clone();
            m.log_compress = current.log_compress;
        }
        _ => m.matchers = current.matchers.clone(),
    }
    m
}

/// 统一的字段写入辅助（保持 prop:value 绑定与 dirty 标记的成对出现）
pub fn set_form_value(f: RwSignal<FormState>, mutate: impl FnOnce(&mut FormState)) {
    f.update(mutate);
}
