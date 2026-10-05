//! 只读终端仿真（FR-6 / HLD D-3）：后端用 `vte` 解析 PTY 输出并维护屏幕栅格，
//! 前端仅绘制序列化后的屏幕帧（带样式 run），不做终端状态机。
//! 栅格状态机见 emu.rs，vte 回调见 perform.rs。
//!
//! MVP 支持的基本序列（FR-6.1）：
//! - SGR 颜色/加粗/下划线/反显（含 256 色、24bit RGB）
//! - CUP/HVP、CUU/CUD/CUF/CUB/CNL/CPL/CHA/VPA、ED、EL、ECH、IL/DL
//! - CR/LF/BS/TAB/BEL、RI/IND/NEL、RIS、保存/恢复光标
//! - 自动换行与底部滚动（进度条重绘场景：`\r` + 行内重写）

mod emu;
mod perform;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tokio::sync::Notify;

pub use emu::TermEmulator;

/// 颜色（终端 16/256 色板索引或 RGB）
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Color {
    Idx(u8),
    Rgb(u8, u8, u8),
}

/// 字符样式
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self { fg: None, bg: None, bold: false, italic: false, underline: false, reverse: false }
    }
}

impl Style {
    pub(super) fn is_plain(&self) -> bool {
        self.fg.is_none() && self.bg.is_none() && !self.bold && !self.italic && !self.underline
            && !self.reverse
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Cell {
    pub ch: char,
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Self { ch: ' ', style: Style::default() }
    }
}

/// 带样式的连续文本段（前端直接渲染为 <span>）
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Run {
    pub t: String,
    pub s: Style,
}

/// 一帧屏幕快照
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TermScreen {
    pub rows: u16,
    pub cols: u16,
    /// (row, col)，0 起
    pub cursor: (u16, u16),
    /// 每行若干 run；空行 = 空数组
    pub lines: Vec<Vec<Run>>,
}

/// 反显样式在渲染前换 fg/bg 并补默认色（screen 序列化与 put_char 共用）
pub(super) fn effective_style_of(s: &Style) -> Style {
    let mut s = s.clone();
    if s.reverse {
        std::mem::swap(&mut s.fg, &mut s.bg);
        if s.fg.is_none() {
            s.fg = Some(Color::Idx(7));
        }
        if s.bg.is_none() {
            s.bg = Some(Color::Idx(0));
        }
        s.reverse = false;
    }
    s
}

/// 共享终端句柄：PTY 读线程喂入，帧发射任务与快照查询读取。
pub struct TermHandle {
    emu: Mutex<TermEmulator>,
    parser: Mutex<vte::Parser>,
    dirty: AtomicBool,
    notify: Notify,
}

impl TermHandle {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            emu: Mutex::new(TermEmulator::new(cols, rows)),
            parser: Mutex::new(vte::Parser::new()),
            dirty: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }

    /// 喂入 PTY 原始字节（线程安全，std 线程调用）
    pub fn feed(&self, bytes: &[u8]) {
        {
            let mut parser = self.parser.lock().unwrap();
            let mut emu = self.emu.lock().unwrap();
            for &b in bytes {
                parser.advance(&mut *emu, b);
            }
        }
        self.dirty.store(true, Ordering::Relaxed);
        self.notify.notify_one();
    }

    /// 清屏复位（FR-6.6 会话隔离）：服务每次启动时调用，
    /// 栅格与解析器状态归零，上一会话输出不保留；并唤醒帧发射任务推送空屏。
    pub fn clear(&self) {
        {
            let mut parser = self.parser.lock().unwrap();
            let mut emu = self.emu.lock().unwrap();
            *parser = vte::Parser::new();
            let (cols, rows) = emu.size();
            *emu = TermEmulator::new(cols, rows);
        }
        self.dirty.store(true, Ordering::Relaxed);
        self.notify.notify_one();
    }

    pub fn has_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    pub async fn wait_output(&self) {
        self.notify.notified().await;
    }

    /// 取当前屏幕并清除脏标记（帧发射任务用）
    pub fn take_screen(&self) -> TermScreen {
        self.dirty.store(false, Ordering::Relaxed);
        self.emu.lock().unwrap().screen()
    }

    pub fn screen(&self) -> TermScreen {
        self.emu.lock().unwrap().screen()
    }
}
