//! 与 crates/core 序列化结构一一对应的 DTO 镜像（serde 字段名/大小写对齐）。
//!
//! 字段与 core 对齐但前端未必全部读取，故不提示 dead_code。

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct EnvEntry {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub expand: bool,
    /// 行级平台标签（FR-2.13）：None = 全部平台
    #[serde(default)]
    pub platform: Option<String>,
}

/// 平台变体（FR-2.13 / DESIGN §4.2）：命令行/工作目录/捕获模式/交互终端的
/// 平台覆盖；None / 空串 = 跟随默认值
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PlatformVariantDto {
    #[serde(default)]
    pub command_line: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub spawn_mode: Option<String>,
    #[serde(default)]
    pub term_interactive: Option<bool>,
}

/// 平台注册表项（get_platform_info）
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PlatformDefDto {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub family: Option<String>,
}

/// get_platform_info 返回：平台下拉选项、本机角标与解析链提示（DESIGN §4.7）
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PlatformInfoDto {
    #[serde(default)]
    pub native_os: String,
    #[serde(default)]
    pub platforms: Vec<PlatformDefDto>,
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub chain: Vec<String>,
}

/// parse_pasted_line 返回（FR-2.12）：前缀环境变量 + 剩余命令行
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PastedLineDto {
    #[serde(default)]
    pub env: Vec<EnvEntry>,
    #[serde(default)]
    pub command_line: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MatcherConfig {
    pub pattern: String,
    #[serde(default)]
    pub template: String,
    #[serde(default = "default_max_items")]
    pub max_items: u32,
}

fn default_max_items() -> u32 {
    5
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LogFilePolicyDto {
    pub path: String,
    #[serde(default = "default_max_bytes")]
    pub max_bytes: u64,
    #[serde(default = "default_max_files")]
    pub max_files: u32,
    #[serde(default)]
    pub compress: bool,
}

fn default_max_bytes() -> u64 {
    10_485_760
}

fn default_max_files() -> u32 {
    3
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LogPolicyDto {
    pub file: Option<LogFilePolicyDto>,
}

/// core ServiceConfig 镜像（枚举以字符串承载：No/OnFailure/Always、Pipe/Pty）
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ServiceConfigDto {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: Vec<EnvEntry>,
    #[serde(default)]
    pub redirect_stderr: bool,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub autorestart: String,
    #[serde(default = "d3")]
    pub startsecs: u64,
    #[serde(default = "d5")]
    pub startretries: u32,
    #[serde(default = "d10")]
    pub stop_timeout_secs: u64,
    #[serde(default)]
    pub spawn_mode: String,
    /// 经用户登录 shell 启动（继承 .zshrc/.bashrc 环境，CONFIG-SPEC §3.5）
    #[serde(default)]
    pub shell: bool,
    /// 终端视图可写（交互，仅 Pty 有效，SRS FR-6.4）
    #[serde(default)]
    pub term_interactive: bool,
    #[serde(default)]
    pub log: LogPolicyDto,
    #[serde(default)]
    pub matchers: Vec<MatcherConfig>,
    /// 基础配置字段的平台变体（键 ∈ 平台注册表，FR-2.13）
    #[serde(default)]
    pub platform: BTreeMap<String, PlatformVariantDto>,
}

fn d3() -> u64 {
    3
}
fn d5() -> u32 {
    5
}
fn d10() -> u64 {
    10
}

#[derive(Debug, Clone, Deserialize)]
pub struct MatchItem {
    /// JS 安全整数（≤ 2^53-1，core 生成时截断；见 docs/DEV-CONVENTIONS.md §2）
    pub hash: u64,
    pub label: String,
    pub url: Option<String>,
}

/// src-tauri list_services 返回（brief 扁平化 + command_line）
///
/// 注意：`config` 为**嵌套**对象（与 core `ServiceBrief` 逐字段对齐，HLD D-14）。
/// 曾误用 `#[serde(flatten)]`：serde 对嵌套对象静默降级为默认值（全部字段为空），
/// 导致列表无名、编辑不回显且无任何报错——禁用 flatten。
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceBriefOut {
    pub config: ServiceConfigDto,
    pub state: String,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub reason: Option<serde_json::Value>,
    #[serde(default)]
    pub since_ms: u64,
    #[serde(default)]
    pub match_items: Vec<MatchItem>,
    pub command_line: String,
    /// 当前平台生效命令行（DESIGN §4.2）：编辑页「本机实际执行」提示用
    #[serde(default)]
    pub effective_command_line: String,
    /// 平台变体回显（含非本机平台，存盘无损）
    #[serde(default)]
    pub platform: BTreeMap<String, PlatformVariantDto>,
    /// 日志单文件上限的带单位展示值（如 `10MB`；未启用文件日志为空，FR-2.2 / D-16）
    #[serde(default)]
    pub log_max_bytes_text: String,
}

impl ServiceBriefOut {
    pub fn display_name(&self) -> &str {
        if self.config.name.is_empty() {
            &self.config.id
        } else {
            &self.config.name
        }
    }

    /// 状态圆点 CSS 类（FR-1.5 色彩语义）
    pub fn state_class(&self) -> &'static str {
        match self.state.as_str() {
            "RUNNING" => "ok",
            "STARTING" | "STOPPING" | "BACKOFF" => "warn",
            "EXITED" | "FATAL" => "bad",
            _ => "muted",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogLine {
    pub seq: u64,
    pub ts: String,
    pub stream: String,
    pub text: String,
}

/// upsert / 预览入参（backend ServiceInput 镜像）
#[derive(Debug, Clone, Serialize, Default)]
pub struct ServiceInput {
    pub id: String,
    pub name: String,
    pub command_line: String,
    pub cwd: Option<String>,
    pub env: Vec<EnvEntry>,
    pub redirect_stderr: bool,
    pub autostart: bool,
    pub autorestart: String,
    pub startsecs: u64,
    pub startretries: u32,
    pub stopwaitsecs: u64,
    pub spawn_mode: String,
    pub shell: bool,
    pub term_interactive: bool,
    pub log_file_enabled: bool,
    pub log_path: Option<String>,
    /// 带单位大小（`10MB` / `512KB` / `1GB` 或纯字节；解析在后端，FR-2.2 / D-16）
    pub log_max_bytes: String,
    pub log_max_files: u32,
    pub log_compress: bool,
    pub matchers: Vec<MatcherConfig>,
    /// 基础配置字段的平台变体（FR-2.13）
    #[serde(default)]
    pub platform: BTreeMap<String, PlatformVariantDto>,
}

// ---------- 终端屏幕（core::term 序列化镜像） ----------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorDto {
    Idx(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct StyleDto {
    pub fg: Option<ColorDto>,
    pub bg: Option<ColorDto>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default)]
    pub reverse: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunDto {
    pub t: String,
    pub s: StyleDto,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TermScreenDto {
    pub rows: u16,
    pub cols: u16,
    pub cursor: (u16, u16),
    pub lines: Vec<Vec<RunDto>>,
}

/// xterm 256 色板（16 基础色 + 6×6×6 立方 + 24 级灰阶）
pub fn palette(idx: u8) -> String {
    const BASE: [&str; 16] = [
        "#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5",
        "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#ffffff",
    ];
    if idx < 16 {
        return BASE[idx as usize].to_string();
    }
    if idx < 232 {
        let i = idx - 16;
        let steps = [0u8, 95, 135, 175, 215, 255];
        let r = steps[(i / 36) as usize];
        let g = steps[((i % 36) / 6) as usize];
        let b = steps[(i % 6) as usize];
        return format!("#{r:02x}{g:02x}{b:02x}");
    }
    let gray = 8 + (idx - 232) * 10;
    format!("#{gray:02x}{gray:02x}{gray:02x}")
}

pub fn color_css(c: &ColorDto) -> String {
    match c {
        ColorDto::Idx(i) => palette(*i),
        ColorDto::Rgb(r, g, b) => format!("# {r:02x}{g:02x}{b:02x}").replace(' ', ""),
    }
}

// ---------- 事件载荷镜像（CoreEvent 内部 tag 序列化） ----------

#[derive(Debug, Clone, Deserialize)]
pub struct StateChangedEv {
    pub id: String,
    pub info: StateInfoDto,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StateInfoDto {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogAppendedEv {
    pub id: String,
    pub lines: Vec<LogLine>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TermFrameEv {
    pub id: String,
    pub screen: TermScreenDto,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigInvalidEv {
    #[serde(default)]
    pub errors: Vec<String>,
}
