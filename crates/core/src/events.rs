//! 内部广播事件总线（HLD §1.2 原则 3）：托盘菜单重建与前端事件推送共用同一事件源。
//!
//! 对外（Tauri bridge）映射为 §4.2 事件名：
//! service-state-changed / log-appended / matcher-hit / config-changed
//! （另加 term-frame、config-invalid）。

use serde::Serialize;
use tokio::sync::broadcast;

use crate::config::ConfigDelta;
use crate::term::TermScreen;

/// 服务生命周期状态（FR-1.5 / FR-3.1，对齐 Supervisor 语义）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SvcState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Backoff,
    Exited,
    Fatal,
}

impl SvcState {
    /// 菜单状态文案（FR-1.5）
    pub fn as_str(&self) -> &'static str {
        match self {
            SvcState::Stopped => "STOPPED",
            SvcState::Starting => "STARTING",
            SvcState::Running => "RUNNING",
            SvcState::Stopping => "STOPPING",
            SvcState::Backoff => "BACKOFF",
            SvcState::Exited => "EXITED",
            SvcState::Fatal => "FATAL",
        }
    }

    /// 状态色（FR-1.5）：灰/黄/绿/黄/橙/红/红
    pub fn color_rgb(&self) -> (u8, u8, u8) {
        match self {
            SvcState::Stopped => (0x9e, 0x9e, 0x9e),
            SvcState::Starting | SvcState::Stopping => (0xf1, 0xc4, 0x0f),
            SvcState::Running => (0x2e, 0xcc, 0x71),
            SvcState::Backoff => (0xe6, 0x7e, 0x22),
            SvcState::Exited | SvcState::Fatal => (0xe7, 0x4c, 0x3c),
        }
    }

    pub fn css_class(&self) -> &'static str {
        match self {
            SvcState::Running => "ok",
            SvcState::Starting | SvcState::Stopping | SvcState::Backoff => "warn",
            SvcState::Exited | SvcState::Fatal => "bad",
            SvcState::Stopped => "muted",
        }
    }

    pub fn is_alive(&self) -> bool {
        matches!(self, SvcState::Starting | SvcState::Running | SvcState::Stopping)
    }
}

/// 停止原因（FR-3.5）：正常 / 超时强杀 / 信号 / 启动失败
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopReason {
    /// 退出码 0 或按停止流程正常退出
    Normal,
    /// 超时（stopwaitsecs）后被强杀
    ForcedTimeout,
    /// 被信号终止（Unix），payload = 信号编号
    Signal(i32),
    /// spawn 失败（可执行文件不存在等）
    SpawnError(String),
}

impl StopReason {
    pub fn describe(&self) -> String {
        match self {
            StopReason::Normal => "正常退出".to_string(),
            StopReason::ForcedTimeout => "超时强杀".to_string(),
            StopReason::Signal(s) => format!("被信号 {s} 终止"),
            StopReason::SpawnError(e) => format!("启动失败：{e}"),
        }
    }
}

/// 日志流标识（FR-4.1）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Stream {
    Out,
    Err,
}

/// 单条日志（合并后带时间戳与流标识）
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    /// 全服务单调递增序号（get_log since_index 用）
    pub seq: u64,
    /// 本地时间 "YYYY-MM-DD HH:MM:SS.mmm"
    pub ts: String,
    pub stream: Stream,
    pub text: String,
}

/// Matcher 命中的快捷项（FR-5.2）
#[derive(Debug, Clone, Serialize)]
pub struct MatchItem {
    /// 以匹配结果哈希为键（去重 + 菜单项 ID 编码 match:<svc_id>:<hash>）
    pub hash: u64,
    /// 模板渲染后的菜单文案
    pub label: String,
    /// http/https 链接；非链接项为 None（菜单中禁用，FR-5.6）
    pub url: Option<String>,
}

/// 运行态快照（状态机对外投影）
#[derive(Debug, Clone, Serialize)]
pub struct StateInfo {
    pub state: SvcState,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub reason: Option<StopReason>,
    /// 进入当前状态的 UNIX 毫秒时间戳（运行时长计算用）
    pub since_ms: u64,
}

/// 核心事件（HLD §4.2 + 终端帧）
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CoreEvent {
    StateChanged { id: String, info: StateInfo },
    LogAppended { id: String, lines: Vec<LogLine> },
    MatcherHit { svc_id: String, items: Vec<MatchItem> },
    ConfigChanged { delta: ConfigDelta },
    /// 热加载校验失败（FR-2.6：沿用上一次有效配置并提示）
    ConfigInvalid { errors: Vec<String> },
    TermFrame { id: String, screen: TermScreen },
}

/// 事件广播总线
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<CoreEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.tx.subscribe()
    }

    pub fn send(&self, ev: CoreEvent) {
        // 无订阅者时丢弃是正常的（例如还没有前端窗口）
        let _ = self.tx.send(ev);
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
