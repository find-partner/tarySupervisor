//! 进程 spawn/stop 平台分支（HLD §1.2 原则 4：平台差异集中于此）。
//!
//! - 管道模式：
//!   - Unix：`pre_exec(setsid)` 新会话（进程组 = 子进程），killpg 优雅/强杀（unix.rs）；
//!   - Windows：`CreateProcessW(CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP)` →
//!     `AssignProcessToJobObject(KILL_ON_JOB_CLOSE)` → `ResumeThread`
//!     （spike `windows/job-object-kill-tree` 已验证的时序，保证子进程无逃逸窗口；
//!     windows.rs + winapi.rs）。
//! - PTY 模式：portable-pty（Unix openpty+setsid / Windows ConPTY）。
//!
//! FR-3.6：全程不经 shell 包装，参数按数组直接传给目标程序。

use std::io::{Read, Write}; // Write 供 RunCtx.pty_writer（FR-6.4 预留输入通道）
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::oneshot;

use super::HubMsg;
// 供子模块（unix.rs / windows.rs）以 super::TermHandle 引用
use super::TermHandle;
use crate::config::ServiceConfig;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod winapi;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix::{spawn_pipe, spawn_pty};
#[cfg(windows)]
use windows::{spawn_pipe, spawn_pty};

/// 退出信息：code 为 None 表示被信号终止（Unix）
#[derive(Debug, Clone)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

impl ExitInfo {
    pub fn is_success(&self) -> bool {
        self.code == Some(0) && self.signal.is_none()
    }
}

/// 强杀/优雅停止句柄（闭包封装，平台差异内聚）
#[derive(Clone)]
pub struct Killer {
    graceful: Arc<dyn Fn() + Send + Sync>,
    force: Arc<dyn Fn() + Send + Sync>,
}

impl Killer {
    pub fn new(
        graceful: impl Fn() + Send + Sync + 'static,
        force: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self { graceful: Arc::new(graceful), force: Arc::new(force) }
    }

    pub fn graceful(&self) {
        (self.graceful)()
    }

    pub fn force(&self) {
        (self.force)()
    }
}

/// 一次成功 spawn 的运行上下文
pub struct RunCtx {
    pub pid: u32,
    pub killer: Killer,
    /// 阻塞等待线程在进程退出后发送（oneshot）
    pub exit: oneshot::Receiver<ExitInfo>,
    /// PTY master 写端（FR-6.4 预留 stdin 通道；MVP 只读，不暴露 UI）
    pub pty_writer: Option<Box<dyn Write + Send>>,
}

pub(crate) fn default_cwd(cfg: &ServiceConfig) -> PathBuf {
    cfg.cwd.clone().unwrap_or_else(|| {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
    })
}

/// spawn 总入口：按 spawn_mode 分派；读线程在此启动并把行送入 hub。
/// 服务未显式配置 PATH 时在此注入登录 shell PATH 兜底（GUI 宿主 PATH 精简，
/// 见 shellenv 模块说明）；对 Pipe / Pty、Unix / Windows 统一生效。
pub(crate) fn spawn_service(
    cfg: &ServiceConfig,
    env: &std::collections::BTreeMap<String, String>,
    term: Option<&Arc<super::TermHandle>>,
    hub_tx: &tokio::sync::mpsc::UnboundedSender<HubMsg>,
) -> Result<RunCtx, String> {
    let mut env = env.clone();
    super::shellenv::inject_fallback_path(&mut env);
    match cfg.spawn_mode {
        crate::config::SpawnMode::Pipe => spawn_pipe(cfg, &env, hub_tx),
        crate::config::SpawnMode::Pty => spawn_pty(cfg, &env, term, hub_tx),
    }
}

/// 目标命令解析：`shell=true` 时包装为用户 shell 的 argv，否则原样直传（FR-3.6）。
pub(super) fn resolve_command(cfg: &ServiceConfig) -> (String, Vec<String>) {
    if !cfg.shell {
        return (cfg.command.clone(), cfg.args.clone());
    }
    let line = std::iter::once(cfg.command.as_str())
        .chain(cfg.args.iter().map(String::as_str))
        .map(|tok| {
            #[cfg(unix)]
            { crate::config::shell_quote_posix(tok) }
            #[cfg(windows)]
            { winapi::quote_arg(tok) }
        })
        .collect::<Vec<_>>()
        .join(" ");
    #[cfg(unix)]
    {
        // -i -l：.zshrc/.bashrc（nvm 等常驻初始化所在地）只有在交互/登录 shell 生效
        (super::shellenv::shell_binary(), vec!["-ilc".into(), line])
    }
    #[cfg(windows)]
    {
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
        (comspec, vec!["/d".into(), "/s".into(), "/c".into(), line])
    }
}

// ---------------------------------------------------------------------------
// 管道读线程：字节流 → 行 → hub
// ---------------------------------------------------------------------------

pub(super) fn spawn_reader_thread(
    mut read: Box<dyn Read + Send>,
    stream: crate::events::Stream,
    tag_as_out: bool,
    hub_tx: tokio::sync::mpsc::UnboundedSender<HubMsg>,
) {
    std::thread::spawn(move || {
        let mut decoder = crate::log::LineDecoder::new();
        let mut buf = [0u8; 8192];
        loop {
            match read.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let effective = if tag_as_out { crate::events::Stream::Out } else { stream };
                    for line in decoder.feed(&buf[..n]) {
                        if hub_tx.send(HubMsg::Line(effective, line)).is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    log::warn!("日志读取失败：{e}");
                    break;
                }
            }
        }
        if let Some(line) = decoder.finish() {
            let _ = hub_tx.send(HubMsg::Line(
                if tag_as_out { crate::events::Stream::Out } else { stream },
                line,
            ));
        }
    });
}

pub(super) fn spawn_pty_reader_thread(
    mut read: Box<dyn Read + Send>,
    term: Arc<super::TermHandle>,
    hub_tx: tokio::sync::mpsc::UnboundedSender<HubMsg>,
) {
    std::thread::spawn(move || {
        let mut decoder = crate::log::LineDecoder::new();
        let mut buf = [0u8; 8192];
        loop {
            match read.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    term.feed(&buf[..n]);
                    for line in decoder.feed(&buf[..n]) {
                        if hub_tx.send(HubMsg::Line(crate::events::Stream::Out, line)).is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    log::warn!("PTY 读取失败：{e}");
                    break;
                }
            }
        }
        if let Some(line) = decoder.finish() {
            let _ = hub_tx.send(HubMsg::Line(crate::events::Stream::Out, line));
        }
    });
}
