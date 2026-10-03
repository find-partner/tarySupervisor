//! TraySupervisor 纯 Rust 核心（HLD §1.2 设计原则 1：不依赖 tauri，可独立单测）。
//!
//! 模块划分对应 HLD §3：
//! - [`config`]：supervisord 风格 INI 配置（CONFIG-SPEC）的模型、解析、校验、env 展开
//! - [`log`]：日志管道（环形缓冲 + 轮转文件 + 行合并）
//! - [`matcher`]：日志正则 → 托盘快捷项（FR-5）
//! - [`term`]：vte 只读终端仿真（FR-6，HLD D-3 后端渲染）
//! - [`service`]：服务状态机与进程托管（FR-3）
//! - [`events`]：内部广播事件总线（托盘与前端共用同一事件源）
//! - [`watcher`]：配置文件热加载（FR-2.6）

pub mod config;
pub mod events;
pub mod log;
pub mod matcher;
pub mod service;
pub mod term;
pub mod watcher;

pub use events::{CoreEvent, LogLine, MatchItem, SvcState, StopReason, Stream};
pub use service::{ServiceBrief, ServiceManager};

use std::path::PathBuf;

/// 配置目录：`~/.traysupervisor`（CONFIG-SPEC §1）
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".traysupervisor")
}

/// 配置文件路径：`~/.traysupervisor/config.ini`
pub fn config_path() -> PathBuf {
    config_dir().join("config.ini")
}

/// 展开 `~` 前缀为用户主目录（CONFIG-SPEC §3.1 directory / stdout_logfile）。
pub fn expand_tilde(p: &str) -> PathBuf {
    if p == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(p));
    }
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}
