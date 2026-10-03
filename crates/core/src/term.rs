//! 只读终端仿真（FR-6 / HLD D-3）：后端用 `vte` 解析 PTY 输出并维护屏幕栅格，
//! 前端仅绘制序列化后的屏幕帧（带样式 run），不做终端状态机。
//!
//! MVP 支持的基本序列（FR-6.1）：
//! - SGR 颜色/加粗/下划线/反显（含 256 色、24bit RGB）
//! - CUP/HVP、CUU/CUD/CUF/CUB/CNL/CPL/CHA/VPA、ED、EL、ECH、IL/DL
//! - CR/LF/BS/TAB/BEL、RI/IND/NEL、RIS、保存/恢复光标
//! - 自动换行与底部滚动（进度条重绘场景：`\r` + 行内重写）

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tokio::sync::Notify;
use vte::{Parser, Perform};

/// 颜色（终端 16/256 色板索引或 RGB）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Color {
    Idx(u8),
    Rgb(u8, u8, u8),
}

/// 字符样式
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    fn is_plain(&self) -> bool {
        self.fg.is_none() && self.bg.is_none() && !self.bold && !self.italic && !self.underline
            && !self.reverse
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Cell {
    ch: char,
    style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Self { ch: ' ', style: Style::default() }
    }
}

/// 带样式的连续文本段（前端直接渲染为 <span>）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Run {
    pub t: String,
    pub s: Style,
}

/// 一帧屏幕快照
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermScreen {
    pub rows: u16,
    pub cols: u16,
    /// (row, col)，0 起
    pub cursor: (u16, u16),
    /// 每行若干 run；空行 = 空数组
    pub lines: Vec<Vec<Run>>,
}

/// vte 解析回调 → 栅格状态机
pub struct TermEmulator {
    cols: u16,
    rows: u16,
    grid: Vec<Vec<Cell>>,
    cursor: (u16, u16),
    style: Style,
    saved: Option<(u16, u16)>,
}

impl TermEmulator {
    pub fn new(cols: u16, rows: u16) -> Self {
        let grid = vec![vec![Cell::default(); cols as usize]; rows as usize];
        Self { cols, rows, grid, cursor: (0, 0), style: Style::default(), saved: None }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// 序列化当前屏幕（合并同样式 run，行尾空白裁剪）
    pub fn screen(&self) -> TermScreen {
        let lines = self
            .grid
            .iter()
            .map(|row| {
                let mut runs: Vec<Run> = Vec::new();
                let mut cur_style: Option<Style> = None;
                let mut cur_text = String::new();
                // 找行尾最后一个非空白（默认样式）位置
                let mut end = row.len();
                while end > 0 && row[end - 1].ch == ' ' && row[end - 1].style.is_plain() {
                    end -= 1;
                }
                for cell in &row[..end] {
                    let st = effective_style(cell);
                    if Some(&st) != cur_style.as_ref() {
                        if !cur_text.is_empty() {
                            runs.push(Run { t: std::mem::take(&mut cur_text), s: cur_style.take().unwrap_or_default() });
                        }
                        cur_style = Some(st);
                    }
                    cur_text.push(cell.ch);
                }
                if !cur_text.is_empty() {
                    runs.push(Run { t: cur_text, s: cur_style.unwrap_or_default() });
                }
                runs
            })
            .collect();
        TermScreen { rows: self.rows, cols: self.cols, cursor: self.cursor, lines }
    }

    fn blank_row(&self) -> Vec<Cell> {
        vec![Cell::default(); self.cols as usize]
    }

    fn scroll_up(&mut self, n: u16) {
        for _ in 0..n {
            self.grid.remove(0);
            self.grid.push(self.blank_row());
        }
    }

    fn scroll_down(&mut self, n: u16) {
        for _ in 0..n {
            self.grid.insert(0, self.blank_row());
            self.grid.pop();
        }
    }

    fn linefeed(&mut self) {
        let (r, _) = self.cursor;
        if r + 1 >= self.rows {
            self.scroll_up(1);
            self.cursor.0 = self.rows - 1;
        } else {
            self.cursor.0 = r + 1;
        }
    }

    fn put_char(&mut self, c: char) {
        let (r, cidx) = self.cursor;
        let style = effective_style_of(&self.style);
        self.grid[r as usize][cidx as usize] = Cell { ch: c, style };
        self.cursor.1 += 1;
        if self.cursor.1 >= self.cols {
            self.cursor.1 = 0;
            self.linefeed();
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        let (r, c) = self.cursor;
        let _ = c;
        let row = &mut self.grid[r as usize];
        let blank = Cell::default();
        match mode {
            0 => for i in c as usize..row.len() { row[i] = blank.clone(); },
            1 => for i in 0..=c.min(self.cols - 1) as usize { row[i] = blank.clone(); },
            _ => for i in 0..row.len() { row[i] = blank.clone(); },
        }
    }

    fn erase_in_display(&mut self, mode: u16) {
        let (r, _c) = self.cursor;
        match mode {
            0 => {
                self.erase_in_line(0);
                let blank = self.blank_row();
                for row in self.grid.iter_mut().skip(r as usize + 1) {
                    *row = blank.clone();
                }
            }
            1 => {
                self.erase_in_line(1);
                let blank = self.blank_row();
                for row in self.grid.iter_mut().take(r as usize) {
                    *row = blank.clone();
                }
            }
            _ => {
                let blank = self.blank_row();
                for row in self.grid.iter_mut() {
                    *row = blank.clone();
                }
            }
        }
    }

    fn apply_sgr(&mut self, codes: &[u16]) {
        let mut i = 0;
        while i < codes.len() {
            let code = codes[i];
            match code {
                0 => self.style = Style::default(),
                1 => self.style.bold = true,
                3 => self.style.italic = true,
                4 => self.style.underline = true,
                7 => self.style.reverse = true,
                22 => { self.style.bold = false; }
                23 => self.style.italic = false,
                24 => self.style.underline = false,
                27 => self.style.reverse = false,
                30..=37 => self.style.fg = Some(Color::Idx((code - 30) as u8)),
                39 => self.style.fg = None,
                40..=47 => self.style.bg = Some(Color::Idx((code - 40) as u8)),
                49 => self.style.bg = None,
                90..=97 => self.style.fg = Some(Color::Idx((code - 90 + 8) as u8)),
                100..=107 => self.style.bg = Some(Color::Idx((code - 100 + 8) as u8)),
                38 | 48 => {
                    // 扩展色：38;5;n / 38;2;r;g;b（可能以子参数形式到达，已在调用方摊平）
                    let color = if codes.get(i + 1) == Some(&5) {
                        codes.get(i + 2).map(|n| Color::Idx(*n as u8))
                    } else if codes.get(i + 1) == Some(&2) {
                        match (codes.get(i + 2), codes.get(i + 3), codes.get(i + 4)) {
                            (Some(&r), Some(&g), Some(&b)) => Some(Color::Rgb(r as u8, g as u8, b as u8)),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    if code == 38 {
                        self.style.fg = color;
                    } else {
                        self.style.bg = color;
                    }
                    let consumed = if codes.get(i + 1) == Some(&5) { 2 } else if codes.get(i + 1) == Some(&2) { 4 } else { 0 };
                    i += consumed;
                }
                _ => {}
            }
            i += 1;
        }
    }
}

fn effective_style(cell: &Cell) -> Style {
    let mut s = cell.style.clone();
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

fn effective_style_of(s: &Style) -> Style {
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

impl Perform for TermEmulator {
    fn print(&mut self, c: char) {
        self.put_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => {} // BEL
            0x08 => self.cursor.1 = self.cursor.1.saturating_sub(1), // BS
            0x09 => {
                // TAB：8 列制表
                let next = ((self.cursor.1 / 8) + 1) * 8;
                self.cursor.1 = next.min(self.cols - 1);
            }
            0x0A | 0x0B | 0x0C => self.linefeed(),
            0x0D => self.cursor.1 = 0, // CR
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &vte::Params, _intermediates: &[u8], _ignore: bool, action: char) {
        let items: Vec<Vec<u16>> = params.iter().map(|p| p.to_vec()).collect();
        let get = |i: usize, default: u16| -> u16 {
            items.get(i).and_then(|p| p.first()).copied().filter(|&v| v != 0).unwrap_or(default)
        };
        let (r, c) = self.cursor;
        match action {
            'A' => self.cursor.0 = r.saturating_sub(get(0, 1).max(1)),
            'B' | 'e' => self.cursor.0 = (r + get(0, 1).max(1)).min(self.rows - 1),
            'C' | 'a' => self.cursor.1 = (c + get(0, 1).max(1)).min(self.cols - 1),
            'D' => self.cursor.1 = c.saturating_sub(get(0, 1).max(1)),
            'E' => { self.cursor.0 = (r + get(0, 1).max(1)).min(self.rows - 1); self.cursor.1 = 0; }
            'F' => { self.cursor.0 = r.saturating_sub(get(0, 1).max(1)); self.cursor.1 = 0; }
            'G' | '`' => self.cursor.1 = get(0, 1).max(1).saturating_sub(1).min(self.cols - 1),
            'd' => self.cursor.0 = get(0, 1).max(1).saturating_sub(1).min(self.rows - 1),
            'H' | 'f' => {
                self.cursor.0 = get(0, 1).max(1).saturating_sub(1).min(self.rows - 1);
                self.cursor.1 = get(1, 1).max(1).saturating_sub(1).min(self.cols - 1);
            }
            'J' => self.erase_in_display(get(0, 0)),
            'K' => self.erase_in_line(get(0, 0)),
            'X' => {
                let n = get(0, 1).max(1) as usize;
                let row = &mut self.grid[r as usize];
                for i in c as usize..((c as usize + n).min(row.len())) {
                    row[i] = Cell::default();
                }
            }
            'L' => {
                // IL：光标行起插入 n 空行
                let n = get(0, 1).max(1) as usize;
                for _ in 0..n {
                    self.grid.insert(r as usize, self.blank_row());
                    self.grid.pop();
                }
            }
            'M' => {
                // DL：光标行起删除 n 行，下方上移
                let n = get(0, 1).max(1) as usize;
                for _ in 0..n {
                    self.grid.remove(r as usize);
                    self.grid.push(self.blank_row());
                }
            }
            'm' => {
                // SGR：摊平子参数（38:5:x 形式）
                let mut flat: Vec<u16> = Vec::new();
                for p in items {
                    if p.len() == 1 {
                        flat.push(p[0]);
                    } else {
                        flat.extend_from_slice(&p);
                    }
                }
                self.apply_sgr(&flat);
            }
            's' => self.saved = Some(self.cursor),
            'u' => {
                if let Some(s) = self.saved {
                    self.cursor = s;
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'c' => {
                // RIS：全屏复位
                self.grid = vec![self.blank_row(); self.rows as usize];
                self.cursor = (0, 0);
                self.style = Style::default();
            }
            b'7' => self.saved = Some(self.cursor),
            b'8' => {
                if let Some(s) = self.saved {
                    self.cursor = s;
                }
            }
            b'D' => self.linefeed(),                       // IND
            b'E' => { self.cursor.1 = 0; self.linefeed(); } // NEL
            b'M' => {
                // RI：反向下滚
                let (r, _) = self.cursor;
                if r == 0 {
                    self.scroll_down(1);
                } else {
                    self.cursor.0 = r - 1;
                }
            }
            _ => { let _ = intermediates; }
        }
    }
}

/// 共享终端句柄：PTY 读线程喂入，帧发射任务与快照查询读取。
pub struct TermHandle {
    emu: Mutex<TermEmulator>,
    parser: Mutex<Parser>,
    dirty: AtomicBool,
    notify: Notify,
}

impl TermHandle {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            emu: Mutex::new(TermEmulator::new(cols, rows)),
            parser: Mutex::new(Parser::new()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(emu: &mut TermEmulator, s: &str) {
        let mut parser = Parser::new();
        for b in s.as_bytes() {
            parser.advance(emu, *b);
        }
    }

    #[test]
    fn plain_text_and_wrap() {
        let mut emu = TermEmulator::new(10, 3);
        feed(&mut emu, "hello");
        let s = emu.screen();
        assert_eq!(s.lines[0][0].t, "hello");
        feed(&mut emu, "helloworldXYZ"); // 越界自动换行
        let s = emu.screen();
        assert_eq!(s.lines[0][0].t, "hellohello");
        assert_eq!(s.lines[1][0].t, "worldXYZ");
    }

    #[test]
    fn cr_rewrites_progress_line() {
        let mut emu = TermEmulator::new(40, 5);
        feed(&mut emu, "10% [###      ]");
        feed(&mut emu, "\r50% [#####    ]");
        let s = emu.screen();
        assert_eq!(s.lines[0][0].t, "50% [#####    ]");
        assert_eq!(s.lines.len(), 5, "不产生新行");
    }

    #[test]
    fn sgr_colors_and_bold() {
        let mut emu = TermEmulator::new(40, 5);
        feed(&mut emu, "\x1b[1;32mgreen bold\x1b[0m plain");
        let s = emu.screen();
        let r0 = &s.lines[0];
        assert_eq!(r0[0].s.bold, true);
        assert_eq!(r0[0].s.fg, Some(Color::Idx(2)));
        assert_eq!(r0[1].s, Style::default());
        // 256 色
        feed(&mut emu, "\r\x1b[38;5;196mX");
        let s = emu.screen();
        assert_eq!(s.lines[0][0].s.fg, Some(Color::Idx(196)));
        // RGB
        feed(&mut emu, "\r\x1b[38;2;10;20;30mY");
        assert_eq!(emu.screen().lines[0][0].s.fg, Some(Color::Rgb(10, 20, 30)));
    }

    #[test]
    fn cursor_positioning_and_clear() {
        let mut emu = TermEmulator::new(20, 5);
        feed(&mut emu, "AAAAA\r\nBBBBB\r\nCCCCC");
        feed(&mut emu, "\x1b[2J\x1b[H"); // 清屏 + 回原点
        let s = emu.screen();
        assert!(s.lines.iter().all(|l| l.is_empty()));
        assert_eq!(s.cursor, (0, 0));
        feed(&mut emu, "\x1b[3;5HX");
        let s = emu.screen();
        assert_eq!(s.cursor, (2, 5), "打印 X 后光标前进一列");
        let line3 = &s.lines[2];
        // 行首 4 个空白与 X 同为默认样式，合并为一个 run
        assert_eq!(line3.last().unwrap().t, "    X");
    }

    #[test]
    fn scroll_on_bottom_lf() {
        let mut emu = TermEmulator::new(10, 3);
        for i in 0..5 {
            feed(&mut emu, &format!("line{i}\r\n"));
        }
        let s = emu.screen();
        assert_eq!(s.lines[0][0].t, "line3");
        assert_eq!(s.lines[1][0].t, "line4");
    }

    #[test]
    fn reverse_video_swapped() {
        let mut emu = TermEmulator::new(10, 2);
        feed(&mut emu, "\x1b[7mR\x1b[0m");
        let s = emu.screen();
        assert_eq!(s.lines[0][0].s.fg, Some(Color::Idx(7)));
        assert_eq!(s.lines[0][0].s.bg, Some(Color::Idx(0)));
    }

    #[test]
    fn tab_and_backspace() {
        let mut emu = TermEmulator::new(20, 3);
        feed(&mut emu, "a\tb"); // a@0，制表到第 8 列，b@8（光标到 9）
        feed(&mut emu, "\x08X"); // BS → 光标 8，X 覆写 b
        let joined: String = emu
            .screen()
            .lines[0]
            .clone()
            .into_iter()
            .map(|r| r.t)
            .collect();
        assert_eq!(joined, "a       X"); // a + 7 空格 + X
    }

    #[test]
    fn handle_feed_and_snapshot() {
        let h = TermHandle::new(80, 24);
        h.feed("状态：\x1b[32mOK\x1b[0m\r\n".as_bytes());
        assert!(h.has_dirty());
        let s = h.take_screen();
        assert!(!h.has_dirty());
        assert_eq!(s.lines[0].len(), 2, "状态： 与 OK 分为两个 run");
        assert_eq!(s.lines[0][1].s.fg, Some(Color::Idx(2)));
    }
}
