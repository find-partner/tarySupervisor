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

use std::path::{Path, PathBuf};

/// 便携模式数据目录环境变量。PortableApps.com 启动器经 launcher.ini 的
/// `[Environment]` 段注入 `%PAL:DataDir%`（CONFIG-SPEC §1）。
pub const PORTABLE_DATA_ENV: &str = "TRAYSUPERVISOR_DATA_DIR";

/// 便携数据根目录（两处触发条件互斥语义：env 优先，exe 同级 `Data` 目录次之）。
/// 返回 `None` 表示非便携，回落 `~/.traysupervisor`。
fn portable_root(env_data_dir: Option<String>, exe_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = env_data_dir.filter(|s| !s.trim().is_empty()) {
        return Some(PathBuf::from(dir));
    }
    exe_dir.map(|d| d.join("Data")).filter(|p| p.is_dir())
}

/// 配置目录解析优先级（CONFIG-SPEC §1）：
/// 1) `TRAYSUPERVISOR_DATA_DIR` 环境变量（PAL 启动器注入）；
/// 2) exe 同级存在 `Data` 目录（绿色解压包布局）；
/// 3) 回落 `~/.traysupervisor`——安装版与 macOS/Linux 常规形态，行为不变。
pub fn config_dir() -> PathBuf {
    let env_data_dir = std::env::var(PORTABLE_DATA_ENV).ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    portable_root(env_data_dir, exe_dir.as_deref()).unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".traysupervisor")
    })
}

/// 当前是否以便携形态运行（数据目录脱离 `~`）。
/// GUI 侧据此把 WebView2 用户数据目录一并收进便携目录，避免写宿主 LOCALAPPDATA。
pub fn is_portable() -> bool {
    let env_data_dir = std::env::var(PORTABLE_DATA_ENV).ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    portable_root(env_data_dir, exe_dir.as_deref()).is_some()
}

/// 配置文件路径：`~/.traysupervisor/config.ini`
pub fn config_path() -> PathBuf {
    config_dir().join("config.ini")
}

/// 服务文件日志默认路径：`~/.traysupervisor/logs/<服务ID>.log`。
/// GUI 勾选文件日志但路径留空时自动生成（SRS FR-2.2）；INI 的 stdout_logfile
/// 空 = 仅内存语义不变，默认路径只在 GUI 保存通道产生。
pub fn default_log_path(service_id: &str) -> PathBuf {
    config_dir().join("logs").join(format!("{service_id}.log"))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn some_dir(p: &str) -> Option<&Path> {
        Some(Path::new(p))
    }

    #[test]
    fn portable_env_takes_priority_and_blank_means_unset() {
        assert_eq!(
            portable_root(Some("X:\\TrayData".into()), some_dir("C:\\app")),
            Some(PathBuf::from("X:\\TrayData"))
        );
        assert_eq!(portable_root(Some("   ".into()), some_dir("C:\\app")), None);
        assert_eq!(portable_root(Some(String::new()), None), None);
    }

    #[test]
    fn portable_marker_is_exe_adjacent_data_dir() {
        let tmp = std::env::temp_dir().join(format!("tsv-portable-{}", std::process::id()));
        let data = tmp.join("Data");
        std::fs::create_dir_all(&data).unwrap();
        assert_eq!(portable_root(None, Some(&tmp)), Some(data.clone()));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn no_portable_signals_fall_back_to_none() {
        assert_eq!(portable_root(None, some_dir("/definitely/not/here")), None);
        assert_eq!(portable_root(None, None), None);
    }
}
