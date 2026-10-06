//! 配置模块：语义模型（HLD §3.1）+ INI 编解码（CONFIG-SPEC）+ 校验 + env 展开。
//!
//! 内存模型与文件格式分离（CONFIG-SPEC §9）：
//! - 模型里 command 为「程序 + 参数数组」，environment 为有序 KV 表；
//! - 文件里 command 为单行（§3.4 规则切分），environment 为 `K="v"` 串（§4）。

mod cmdline;
mod envstr;
mod ini;
mod validate;

pub use cmdline::{
    join_command_line, shell_quote_posix, split_command_line, split_env_prefixed_line,
};
pub use envstr::{expand_env_value, parse_environment, resolve_env, serialize_environment};
pub use ini::{ensure_default_config, load_app_config, save_app_config};
pub use validate::{
    system_var, validate_app, BACKUPS_MAX, LOG_LINES_MAX, LOG_LINES_MIN, MATCHERS_MAX,
    MATCHER_MAX_ITEMS_MAX, MAX_BYTES_MAX, START_RETRIES_MAX,
};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// 内置平台（CONFIG-SPEC §2 / DESIGN §4.1）：运行时按 `std::env::consts::OS` 自动检测
pub const BUILTIN_PLATFORMS: [&str; 3] = ["windows", "macos", "linux"];

/// 本机运行 OS（与内置平台命名一致：windows / macos / linux）
pub fn native_os() -> &'static str {
    std::env::consts::OS
}

/// 自定义平台声明（CONFIG-SPEC §2）：`platforms=home-mac:macos, office-win:windows`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformDef {
    /// slug，仅 [a-z0-9-_]，不得与内置平台同名
    pub name: String,
    /// OS 家族（内置平台名）：该平台未命中变体时回退到家族平台（可选）
    pub family: Option<String>,
}

/// 全局设置（CONFIG-SPEC §2 [traysupervisor]）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalSettings {
    /// 每服务内存环形缓冲行数（100–100000），默认 2000（FR-4.2）
    pub log_lines: u32,
    /// 服务异常退出时系统通知（P1，模型先保留）
    pub notify_on_failure: bool,
    /// 自定义平台注册表（可选；内置 windows/macos/linux 恒可用）
    #[serde(default)]
    pub platforms: Vec<PlatformDef>,
    /// 当前所在平台（可选；空 = 按内置自动检测。自定义平台必须声明否则其变体不生效）
    #[serde(default)]
    pub current_platform: Option<String>,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            log_lines: 2000,
            notify_on_failure: true,
            platforms: vec![],
            current_platform: None,
        }
    }
}

/// 平台注册表：内置 + 全局声明，提供变体解析链（DESIGN §4.1）。
/// 由合法配置构建；`new` 对非法声明宽容跳过（校验错误由 validate 单独报告）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlatformRegistry {
    custom: Vec<PlatformDef>,
    current: Option<String>,
}

impl PlatformRegistry {
    pub fn new(settings: &GlobalSettings) -> Self {
        let mut custom = Vec::new();
        for def in &settings.platforms {
            let name_ok = !def.name.is_empty()
                && def
                    .name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
                && !BUILTIN_PLATFORMS.contains(&def.name.as_str())
                && custom.iter().all(|d: &PlatformDef| d.name != def.name);
            if name_ok {
                custom.push(def.clone());
            }
        }
        let current = settings
            .current_platform
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        Self { custom, current }
    }

    /// 注册表全部平台名（内置 + 自定义），供 UI 与校验使用
    pub fn all_names(&self) -> Vec<String> {
        BUILTIN_PLATFORMS
            .iter()
            .map(|s| s.to_string())
            .chain(self.custom.iter().map(|d| d.name.clone()))
            .collect()
    }

    pub fn is_known(&self, name: &str) -> bool {
        BUILTIN_PLATFORMS.contains(&name) || self.custom.iter().any(|d| d.name == name)
    }

    /// 平台解析链（按序取第一个命中，均未命中回退默认值，DESIGN §4.1）：
    /// 当前平台 → 其 OS 家族（自定义且声明了家族时）→ 自动检测的内置 OS
    /// （链中尚未含家族时附加；未设 current_platform 时链 = [自动检测 OS]）。
    pub fn chain(&self) -> Vec<String> {
        self.chain_for_opt(self.current.as_deref())
    }

    /// 指定平台的解析链（校验"其他平台"环境变量条目时使用）
    pub fn chain_for(&self, name: &str) -> Vec<String> {
        self.chain_for_opt(Some(name))
    }

    fn chain_for_opt(&self, name: Option<&str>) -> Vec<String> {
        let mut chain: Vec<String> = Vec::new();
        if let Some(n) = name {
            if self.is_known(n) {
                chain.push(n.to_string());
                if let Some(family) = self
                    .custom
                    .iter()
                    .find(|d| d.name == n)
                    .and_then(|d| d.family.as_deref())
                {
                    // 声明了家族：以家族为准，不再附加自动检测 OS
                    if !chain.iter().any(|c| c == family) {
                        chain.push(family.to_string());
                    }
                    return chain;
                }
            }
        }
        let native = native_os();
        if !chain.iter().any(|c| c == native) {
            chain.push(native.to_string());
        }
        chain
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
    /// 基础配置页字段的平台变体（FR-2.13 / DESIGN §4.2）；键 ∈ 平台注册表，
    /// 仅存出现变体的键。基础字段本身 = 默认值（未命中变体时使用）
    #[serde(default)]
    pub platform: BTreeMap<String, PlatformVariant>,
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
            platform: BTreeMap::new(),
        }
    }
}

impl ServiceConfig {
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }

    /// 平台解析链上第一个含该字段变体的取值（DESIGN §4.2）
    fn pick<T>(
        chain: &[String],
        platform: &BTreeMap<String, PlatformVariant>,
        f: impl Fn(&PlatformVariant) -> Option<T>,
    ) -> Option<T> {
        chain.iter().find_map(|p| platform.get(p).and_then(&f))
    }

    /// 解析为「当前平台生效」的配置（spawn / 展示 / 预览统一入口，DESIGN §4.2）：
    /// 命令行、工作目录、捕获模式、交互终端沿解析链取变体覆盖、回退默认；
    /// 环境变量按行级标签过滤合并（§4.3）；platform 表清空（解析结果不回写文件）。
    pub fn resolve_for(&self, reg: &PlatformRegistry) -> ServiceConfig {
        let chain = reg.chain();
        let command = Self::pick(&chain, &self.platform, |v| {
            (!v.command.is_empty()).then_some(v.command.clone())
        })
        .unwrap_or_else(|| self.command.clone());
        let args = Self::pick(&chain, &self.platform, |v| {
            (!v.command.is_empty()).then_some(v.args.clone())
        })
        .unwrap_or_else(|| self.args.clone());
        let cwd = Self::pick(&chain, &self.platform, |v| v.directory.clone())
            .or_else(|| self.cwd.clone());
        let spawn_mode =
            Self::pick(&chain, &self.platform, |v| v.spawn_mode).unwrap_or(self.spawn_mode);
        let term_interactive = Self::pick(&chain, &self.platform, |v| v.term_interactive)
            .unwrap_or(self.term_interactive);
        let env = self.effective_env(&chain);
        ServiceConfig {
            command,
            args,
            cwd,
            env,
            spawn_mode,
            term_interactive,
            platform: BTreeMap::new(),
            ..self.clone()
        }
    }

    /// 环境变量按解析链过滤合并（DESIGN §4.3）：
    /// 保留标签命中（无标签 / 标签 ∈ 链）的条目并保持书写顺序；
    /// 同名键后条目就地覆盖前条目（value 与 expand 均以后者为准、位置不变），
    /// 使后续条目的 `${VAR}` 引用取到最终生效值。
    pub fn effective_env(&self, chain: &[String]) -> Vec<EnvEntry> {
        let matched = self.env.iter().filter(|e| match &e.platform {
            None => true,
            Some(tag) => chain.iter().any(|c| c == tag),
        });
        let mut out: Vec<EnvEntry> = Vec::new();
        for e in matched {
            if let Some(slot) = out.iter_mut().find(|o| o.key == e.key) {
                slot.value = e.value.clone();
                slot.expand = e.expand;
                slot.platform = e.platform.clone();
            } else {
                out.push(e.clone());
            }
        }
        out
    }
}

/// 环境变量条目（FR-2.3）：expand=false 时值字面量直传（默认）。
/// `platform` = 行级平台标签（FR-2.13）：None = 全部平台，Some = 仅该平台生效
/// （解析链含该平台时命中；同名键平台条目就地覆盖全部平台条目，DESIGN §4.3）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EnvEntry {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub expand: bool,
    #[serde(default)]
    pub platform: Option<String>,
}

/// 基础配置字段的平台变体（FR-2.13 / DESIGN §4.2）。
/// command/args 为该平台命令行按 §3.4 切分的结果（command 空 = 未覆盖命令行）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformVariant {
    pub command: String,
    pub args: Vec<String>,
    /// 已展开 ~ 的工作目录；None = 跟随默认
    pub directory: Option<PathBuf>,
    pub spawn_mode: Option<SpawnMode>,
    pub term_interactive: Option<bool>,
}

impl PlatformVariant {
    /// 全部字段均未覆盖（空变体不落盘、不参与解析）
    pub fn is_empty(&self) -> bool {
        self.command.is_empty()
            && self.directory.is_none()
            && self.spawn_mode.is_none()
            && self.term_interactive.is_none()
    }
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
    let digits_end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
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
    n.checked_mul(mult)
        .ok_or_else(|| format!("大小 {s:?} 超出 64 位范围"))
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
    fn registry_chain_builtin_and_custom() {
        // 本机自动检测 OS（测试按运行平台断言链）
        let reg = PlatformRegistry::new(&GlobalSettings::default());
        assert_eq!(reg.chain(), vec![native_os().to_string()]);
        assert!(reg.is_known("macos") && reg.is_known("windows") && reg.is_known("linux"));
        assert!(!reg.is_known("home-mac"));

        // 自定义平台 + 当前平台声明：链 = [自定义, 家族]，不再附加自动检测 OS
        let s = GlobalSettings {
            platforms: vec![
                PlatformDef {
                    name: "home-mac".into(),
                    family: Some("macos".into()),
                },
                PlatformDef {
                    name: "office-win".into(),
                    family: Some("windows".into()),
                },
            ],
            current_platform: Some("home-mac".into()),
            ..Default::default()
        };
        let reg = PlatformRegistry::new(&s);
        assert_eq!(
            reg.chain(),
            vec!["home-mac".to_string(), "macos".to_string()]
        );
        assert_eq!(
            reg.chain_for("office-win"),
            vec!["office-win".to_string(), "windows".to_string()]
        );
        // 无家族的自定义平台：链 = [自身, 自动检测 OS]
        let s2 = GlobalSettings {
            platforms: vec![
                PlatformDef {
                    name: "home-mac".into(),
                    family: Some("macos".into()),
                },
                PlatformDef {
                    name: "office-win".into(),
                    family: Some("windows".into()),
                },
                PlatformDef {
                    name: "build-box".into(),
                    family: None,
                },
            ],
            current_platform: Some("home-mac".into()),
            ..Default::default()
        };
        let reg = PlatformRegistry::new(&s2);
        assert_eq!(
            reg.chain_for("build-box"),
            vec!["build-box".to_string(), native_os().to_string()]
        );
        // 未知 current / 与内置冲突的自定义：宽容跳过（校验层报错）
        let s3 = GlobalSettings {
            platforms: vec![PlatformDef {
                name: "macos".into(),
                family: None,
            }],
            current_platform: Some("nope".into()),
            ..Default::default()
        };
        let reg = PlatformRegistry::new(&s3);
        assert_eq!(reg.chain(), vec![native_os().to_string()]);
    }

    #[test]
    fn resolve_for_folds_variants_along_chain() {
        let s = GlobalSettings {
            platforms: vec![PlatformDef {
                name: "home-mac".into(),
                family: Some("macos".into()),
            }],
            current_platform: Some("home-mac".into()),
            ..Default::default()
        };
        let reg = PlatformRegistry::new(&s);

        let mut svc = ServiceConfig {
            id: "a".into(),
            command: "npx".into(),
            args: vec!["-y".into()],
            ..Default::default()
        };
        svc.platform.insert(
            "macos".into(),
            PlatformVariant {
                command: "/opt/homebrew/bin/ow".into(),
                args: vec!["--port".into(), "1123".into()],
                directory: Some(PathBuf::from("/opt/dir")),
                spawn_mode: Some(SpawnMode::Pty),
                term_interactive: Some(true),
            },
        );
        svc.platform.insert(
            "windows".into(),
            PlatformVariant {
                command: r"C:\tools\ow.exe".into(),
                ..Default::default()
            },
        );

        // 链 [home-mac, macos]：macos 变体命中
        let eff = svc.resolve_for(&reg);
        assert_eq!(eff.command, "/opt/homebrew/bin/ow");
        assert_eq!(eff.spawn_mode, SpawnMode::Pty);
        assert!(eff.term_interactive);
        assert!(eff.platform.is_empty(), "解析结果不携带变体表");
        // 未覆盖字段回退默认
        assert_eq!(eff.autostart, svc.autostart);

        // 链不含 macos/windows（自定义平台声明 linux 家族）→ 全部回退默认
        let s2 = GlobalSettings {
            platforms: vec![PlatformDef {
                name: "ci-box".into(),
                family: Some("linux".into()),
            }],
            current_platform: Some("ci-box".into()),
            ..Default::default()
        };
        let eff = svc.resolve_for(&PlatformRegistry::new(&s2));
        assert_eq!(eff.command, "npx");
        assert_eq!(eff.spawn_mode, SpawnMode::Pipe);
        assert!(!eff.term_interactive);
    }

    #[test]
    fn effective_env_filters_and_overrides_in_place() {
        let mut svc = ServiceConfig {
            id: "a".into(),
            command: "x".into(),
            ..Default::default()
        };
        svc.env = vec![
            EnvEntry {
                key: "PORT".into(),
                value: "3000".into(),
                expand: false,
                platform: None,
            },
            EnvEntry {
                key: "LOG".into(),
                value: "~/logs".into(),
                expand: false,
                platform: None,
            },
            EnvEntry {
                key: "LOG".into(),
                value: r"${LOCALAPPDATA}\logs".into(),
                expand: true,
                platform: Some("windows".into()),
            },
            EnvEntry {
                key: "EXTRA".into(),
                value: "mac-only".into(),
                expand: false,
                platform: Some("macos".into()),
            },
        ];
        // Windows 链：@windows 就地覆盖 LOG（位置不变），@macos 排除
        let eff = svc.effective_env(&["windows".to_string()]);
        assert_eq!(eff.len(), 2);
        assert_eq!(eff[0].key, "PORT");
        assert_eq!(eff[1].key, "LOG");
        assert_eq!(eff[1].value, r"${LOCALAPPDATA}\logs");
        assert!(eff[1].expand);
        // Linux 链：全部平台条目原样
        let eff = svc.effective_env(&["linux".to_string()]);
        assert_eq!(eff.len(), 2);
        assert_eq!(eff[1].value, "~/logs");
        assert!(!eff[1].expand);
        // macos 链：命中 EXTRA
        let eff = svc.effective_env(&["macos".to_string()]);
        assert_eq!(eff.len(), 3);
        assert_eq!(eff[2].key, "EXTRA");
    }

    #[test]
    fn diff_ignores_other_platform_variant_changes() {
        let mut old = vec![ServiceConfig {
            id: "a".into(),
            command: "x".into(),
            ..Default::default()
        }];
        let mut new = old.clone();
        // 改其他平台（假设本机非 windows）的变体 → 生效值不变 → soft
        new[0].platform.insert(
            "windows".into(),
            PlatformVariant {
                command: r"C:\x.exe".into(),
                ..Default::default()
            },
        );
        old[0].platform.insert(
            "windows".into(),
            PlatformVariant {
                command: r"C:\old.exe".into(),
                ..Default::default()
            },
        );
        let reg = PlatformRegistry::new(&GlobalSettings::default());
        let d = diff_configs(&old, &new, &reg, &reg);
        if native_os() == "windows" {
            assert_eq!(d.changed, vec!["a".to_string()]);
        } else {
            assert!(d.changed.is_empty() && d.soft_changed == vec!["a".to_string()]);
        }
        // 改本机命中变体（默认命令行）→ hard
        new[0].command = "y".into();
        let d = diff_configs(&old, &new, &reg, &reg);
        assert_eq!(d.changed, vec!["a".to_string()]);
    }

    #[test]
    fn diff_catches_current_platform_chain_change() {
        // 仅全局节 current_platform 变更（服务节不变）：解析链变化改变生效值 → hard
        let settings = |cur: &str| GlobalSettings {
            platforms: vec![PlatformDef { name: "home-mac".into(), family: Some("macos".into()) }],
            current_platform: Some(cur.into()),
            ..Default::default()
        };
        let mut svc = ServiceConfig { id: "a".into(), command: "default-cmd".into(), ..Default::default() };
        svc.platform.insert(
            "home-mac".into(),
            PlatformVariant { command: "variant-cmd".into(), ..Default::default() },
        );
        let old = vec![svc.clone()];
        let new = vec![svc];
        let d = diff_configs(
            &old,
            &new,
            &PlatformRegistry::new(&settings("home-mac")),
            &PlatformRegistry::new(&settings("build-box")),
        );
        // 旧链 [home-mac, macos] 命中变体；新链 [build-box, macos] 无命中回退默认
        assert_eq!(d.changed, vec!["a".to_string()], "{d:?}");
    }

    #[test]
    fn diff_detects_tagged_env_change_on_other_platform() {
        let mut old = vec![ServiceConfig {
            id: "a".into(),
            command: "x".into(),
            ..Default::default()
        }];
        let mut new = old.clone();
        old[0].env = vec![EnvEntry {
            key: "K".into(),
            value: "1".into(),
            expand: false,
            platform: Some("windows".into()),
        }];
        new[0].env = vec![EnvEntry {
            key: "K".into(),
            value: "2".into(),
            expand: false,
            platform: Some("windows".into()),
        }];
        let reg = PlatformRegistry::new(&GlobalSettings::default());
        let d = diff_configs(&old, &new, &reg, &reg);
        if native_os() == "windows" {
            assert_eq!(d.changed, vec!["a".to_string()]);
        } else {
            assert!(d.changed.is_empty() && d.soft_changed == vec!["a".to_string()]);
        }
    }

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
///
/// 平台变体（FR-2.13 / DESIGN §4.5）：命令行、工作目录、捕获模式、交互终端、环境变量
/// 按「各自解析链解析后的生效值」比较——旧配置按旧注册表、新配置按新注册表
/// （current_platform 变更导致的生效值变化由此识别为 hard change）；
/// 仅其他平台的变体/未命中标签的环境变量行变化 → 本机生效值不变 → 不重启。
pub fn diff_configs(
    old: &[ServiceConfig],
    new: &[ServiceConfig],
    old_reg: &PlatformRegistry,
    new_reg: &PlatformRegistry,
) -> ConfigDelta {
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
            let oe = o.resolve_for(old_reg);
            let ne = s.resolve_for(new_reg);
            // 原始结构相同且按各自解析链解析后的生效值也相同 → 跳过。
            // 快捷路径必须包含解析比较：仅全局节 current_platform/platforms 变更时
            // 服务节本身不变，但解析链变化会改变生效值（DESIGN §4.5），不能省略
            if o == s && oe == ne {
                continue;
            }
            let hard = oe.command != ne.command
                || oe.args != ne.args
                || oe.cwd != ne.cwd
                || oe.env != ne.env
                || oe.spawn_mode != ne.spawn_mode
                || oe.term_interactive != ne.term_interactive
                || o.redirect_stderr != s.redirect_stderr
                || o.autorestart != s.autorestart
                || o.startsecs != s.startsecs
                || o.startretries != s.startretries
                || o.stop_timeout_secs != s.stop_timeout_secs
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
