//! 配置模块：语义模型（HLD §3.1）+ INI 编解码（CONFIG-SPEC）+ 校验 + env 展开。
//!
//! 内存模型与文件格式分离（CONFIG-SPEC §9）：
//! - 模型里 command 为「程序 + 参数数组」，environment 为有序 KV 表；
//! - 文件里 command 为单行（§3.4 规则切分），environment 为 `K="v"` 串（§4）。

mod cmdline;
mod envstr;
mod ini;
mod validate;

pub use cmdline::{join_command_line, shell_quote_posix, split_command_line};
pub use envstr::{expand_env_value, parse_environment, resolve_env, serialize_environment};
pub use ini::{ensure_default_config, load_app_config, save_app_config};
pub use validate::{
    system_var, validate_app, BACKUPS_MAX, LOG_LINES_MAX, LOG_LINES_MIN, MATCHER_MAX_ITEMS_MAX,
    MATCHERS_MAX, MAX_BYTES_MAX, START_RETRIES_MAX,
};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// 全局设置（CONFIG-SPEC §2 [traysupervisor]）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalSettings {
    /// 每服务内存环形缓冲行数（100–100000），默认 2000（FR-4.2）
    pub log_lines: u32,
    /// 服务异常退出时系统通知（P1，模型先保留）
    pub notify_on_failure: bool,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self { log_lines: 2000, notify_on_failure: true }
    }
}

/// 顶层配置
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub settings: GlobalSettings,
    /// 顺序即菜单顺序与启动顺序（CONFIG-SPEC §3）
    pub services: Vec<ServiceConfig>,
}

/// 服务配置（HLD §3.1 模型 + CONFIG-SPEC 字段对照）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// slug，仅 [a-z0-9-_]，全文件唯一；取自节名 [program:<id>]
    pub id: String,
    /// 菜单与 CLI 显示名；默认 = id
    pub name: String,
    /// 程序：可执行文件路径或 PATH 可解析名（不经 shell，FR-3.6）
    pub command: String,
    /// 参数数组，直接传给目标程序
    pub args: Vec<String>,
    /// 工作目录；None = 用户目录
    pub cwd: Option<PathBuf>,
    /// 有序环境变量表；条目间可引用（expand=true 时查排在前面的条目）
    pub env: Vec<EnvEntry>,
    /// true：stderr 并入 stdout 单流（默认 false，保留流标识）
    pub redirect_stderr: bool,
    /// 应用启动时自动启动（默认 false）
    pub autostart: bool,
    /// 自动重启策略（默认 No）
    pub autorestart: Autorestart,
    /// STARTING→RUNNING 判定秒数（默认 3，CONFIG-SPEC startsecs）
    pub startsecs: u64,
    /// BACKOFF 最大重试次数（默认 5，CONFIG-SPEC startretries）
    pub startretries: u32,
    /// 优雅停止等待秒数，超时强杀整树（默认 10；文件键 stopwaitsecs）
    pub stop_timeout_secs: u64,
    /// 捕获方式（默认 Pipe）
    pub spawn_mode: SpawnMode,
    /// true：经用户登录 shell（`$SHELL -ilc`）启动，继承其配置环境
    /// （.zshrc/.bashrc 里的 PATH、nvm 等）；FR-3.6 的显式例外，见 CONFIG-SPEC §3.5
    #[serde(default)]
    pub shell: bool,
    /// 终端视图可写（交互，默认 false；仅 Pty 模式有效，SRS FR-6.4）
    pub term_interactive: bool,
    /// 日志文件策略；None = 仅内存
    pub log: LogPolicy,
    /// Matcher 规则（≤20 条）；空 = 无规则（内置 URL 规则不再隐式启用，FR-5.3：
    /// 仅在新建服务时由 UI 预填并随配置落盘）
    pub matchers: Vec<MatcherConfig>,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            command: String::new(),
            args: vec![],
            cwd: None,
            env: vec![],
            redirect_stderr: false,
            autostart: false,
            autorestart: Autorestart::No,
            startsecs: 3,
            startretries: 5,
            stop_timeout_secs: 10,
            spawn_mode: SpawnMode::Pipe,
            shell: false,
            term_interactive: false,
            log: LogPolicy::default(),
            matchers: vec![],
        }
    }
}

impl ServiceConfig {
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() { &self.id } else { &self.name }
    }
}

/// 环境变量条目（FR-2.3）：expand=false 时值字面量直传（默认）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvEntry {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub expand: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Autorestart {
    No,
    OnFailure,
    Always,
}

impl Default for Autorestart {
    fn default() -> Self {
        Autorestart::No
    }
}

impl Autorestart {
    pub fn as_str(&self) -> &'static str {
        match self {
            Autorestart::No => "no",
            Autorestart::OnFailure => "on-failure",
            Autorestart::Always => "always",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpawnMode {
    Pipe,
    Pty,
}

impl Default for SpawnMode {
    fn default() -> Self {
        SpawnMode::Pipe
    }
}

impl SpawnMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SpawnMode::Pipe => "pipe",
            SpawnMode::Pty => "pty",
        }
    }
}

/// 日志策略（FR-2.2）：内存缓冲必选；file = Some 时写文件并轮转。
/// （CONFIG-SPEC 以 stdout_logfile 是否为空表达；「仅文件」无文件格式表达，MVP 不提供）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LogPolicy {
    pub file: Option<LogFilePolicy>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogFilePolicy {
    /// 已展开 ~ 与相对路径（相对配置文件所在目录）后的绝对路径
    pub path: PathBuf,
    /// 单文件最大字节，0 = 不轮转（CONFIG-SPEC stdout_logfile_maxbytes）
    pub max_bytes: u64,
    /// 轮转分块保留份数 .1 .2 …（stdout_logfile_backups）
    pub max_files: u32,
    /// 旧分块 gzip 压缩（logfile_compress）
    pub compress: bool,
}

/// Matcher 配置（FR-5.1 / CONFIG-SPEC §5）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatcherConfig {
    /// Rust regex 语法
    pub pattern: String,
    /// 菜单文案，{0} {1} 引用捕获组；默认「🌐 打开 {0}」
    pub template: String,
    /// 该条快捷项上限（1–10，LRU 淘汰），默认 5
    pub max_items: u32,
}

impl Default for MatcherConfig {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            template: "🌐 打开 {0}".to_string(),
            max_items: 5,
        }
    }
}

/// 内置 URL Matcher 预设（FR-5.3）：新建服务时由 UI 预填，运行期不隐式启用
pub const BUILTIN_URL_PATTERN: &str = r"https?://[^\s]+";
pub const BUILTIN_URL_TEMPLATE: &str = "🌐 打开 {0}";

/// 解析带单位的字节大小（FR-2.2 / CONFIG-SPEC §3.1）：
/// `10MB`、`512kb`、`1G`、`0`、`10485760` 均可；单位大小写不敏感，`B` 可省略。
/// 返回字节数（内存模型始终为字节，HLD D-16）。
pub fn parse_byte_size(s: &str) -> Result<u64, String> {
    let t = s.trim();
    let digits_end = t
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(t.len());
    let (num, unit) = t.split_at(digits_end);
    if num.is_empty() {
        return Err(format!("大小 {s:?} 缺少数字（示例：10MB、512KB、0）"));
    }
    let mult: u64 = match unit.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1,
        "K" | "KB" => 1024,
        "M" | "MB" => 1024 * 1024,
        "G" | "GB" => 1024 * 1024 * 1024,
        other => {
            return Err(format!(
                "大小 {s:?} 的单位 {other:?} 非法（支持 B/KB/MB/GB 或纯字节数）"
            ))
        }
    };
    let n: u64 = num
        .parse()
        .map_err(|_| format!("大小 {s:?} 的数字部分 {num:?} 非法"))?;
    n.checked_mul(mult).ok_or_else(|| format!("大小 {s:?} 超出 64 位范围"))
}

/// 字节数 → 可读大小字符串（parse_byte_size 的逆操作，用于 INI 回写与 GUI 展示）：
/// 能被 GB/MB/KB 整除时用最大单位，否则输出字节数。
pub fn format_byte_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes == 0 {
        "0".to_string()
    } else if bytes.is_multiple_of(GB) {
        format!("{}GB", bytes / GB)
    } else if bytes.is_multiple_of(MB) {
        format!("{}MB", bytes / MB)
    } else if bytes.is_multiple_of(KB) {
        format!("{}KB", bytes / KB)
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_byte_size_units() {
        assert_eq!(parse_byte_size("10MB").unwrap(), 10_485_760);
        assert_eq!(parse_byte_size("512kb").unwrap(), 512 * 1024);
        assert_eq!(parse_byte_size("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_byte_size("2 GB").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_byte_size("10485760").unwrap(), 10_485_760);
        assert_eq!(parse_byte_size("0").unwrap(), 0);
        assert_eq!(parse_byte_size(" 10MB ").unwrap(), 10_485_760);
        assert_eq!(parse_byte_size("4096B").unwrap(), 4096);
    }

    #[test]
    fn parse_byte_size_rejects_garbage() {
        assert!(parse_byte_size("").is_err());
        assert!(parse_byte_size("MB").is_err());
        assert!(parse_byte_size("10TB").is_err());
        assert!(parse_byte_size("10 MB extra").is_err());
        assert!(parse_byte_size("abc").is_err());
        assert!(parse_byte_size("-5MB").is_err());
    }

    #[test]
    fn format_byte_size_roundtrip() {
        assert_eq!(format_byte_size(10_485_760), "10MB");
        assert_eq!(format_byte_size(512 * 1024), "512KB");
        assert_eq!(format_byte_size(1024 * 1024 * 1024), "1GB");
        assert_eq!(format_byte_size(0), "0");
        assert_eq!(format_byte_size(12345), "12345B");
        for v in [0u64, 1, 4096, 12345, 512 * 1024, 10_485_760, 1024u64.pow(3)] {
            assert_eq!(parse_byte_size(&format_byte_size(v)).unwrap(), v);
        }
    }
}

/// 配置差异（热加载 / reread-update 语义，CONFIG-SPEC §8）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConfigDelta {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// 需要重启才能生效的变更
    pub changed: Vec<String>,
    /// 仅 name / autostart 等即时生效字段变更（不重启）
    pub soft_changed: Vec<String>,
}

impl ConfigDelta {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && self.soft_changed.is_empty()
    }
}

/// diff 旧/新服务配置，产出 ConfigDelta。
///
/// 重启判定字段（CONFIG-SPEC §8）：command / directory / environment / spawn_mode /
/// autorestart / startsecs / stopwaitsecs / stdout_logfile* / logfile_compress / matcher.*
/// （另含 redirect_stderr——同样影响捕获流）；仅 name / autostart 变更即时生效不重启。
pub fn diff_configs(old: &[ServiceConfig], new: &[ServiceConfig]) -> ConfigDelta {
    use std::collections::HashMap;
    let old_map: HashMap<&str, &ServiceConfig> = old.iter().map(|s| (s.id.as_str(), s)).collect();
    let new_map: HashMap<&str, &ServiceConfig> = new.iter().map(|s| (s.id.as_str(), s)).collect();

    let mut delta = ConfigDelta::default();
    for s in new {
        if !old_map.contains_key(s.id.as_str()) {
            delta.added.push(s.id.clone());
        }
    }
    for s in old {
        if !new_map.contains_key(s.id.as_str()) {
            delta.removed.push(s.id.clone());
        }
    }
    for s in new {
        if let Some(o) = old_map.get(s.id.as_str()).copied() {
            if o == s {
                continue;
            }
            let hard = o.command != s.command
                || o.args != s.args
                || o.cwd != s.cwd
                || o.env != s.env
                || o.redirect_stderr != s.redirect_stderr
                || o.autorestart != s.autorestart
                || o.startsecs != s.startsecs
                || o.startretries != s.startretries
                || o.stop_timeout_secs != s.stop_timeout_secs
                || o.spawn_mode != s.spawn_mode
                || o.shell != s.shell
                || o.log != s.log
                || o.matchers != s.matchers;
            if hard {
                delta.changed.push(s.id.clone());
            } else {
                delta.soft_changed.push(s.id.clone());
            }
        }
    }
    delta
}

/// 展开后的 env 预览（编辑页「预览」按钮 / FR-2.4 校验共用）
pub type EnvMap = BTreeMap<String, String>;
