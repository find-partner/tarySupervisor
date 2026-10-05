//! 栅格状态机：vte 解析回调操作的数据结构（光标、滚动、擦除、SGR）。

use super::{effective_style_of, Cell, Color, Run, Style, TermScreen};

/// vte 解析回调 → 栅格状态机
pub struct TermEmulator {
    pub(super) cols: u16,
    pub(super) rows: u16,
    pub(super) grid: Vec<Vec<Cell>>,
    pub(super) cursor: (u16, u16),
    pub(super) style: Style,
    pub(super) saved: Option<(u16, u16)>,
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
                    let st = effective_style_of(&cell.style);
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

    pub(super) fn blank_row(&self) -> Vec<Cell> {
        vec![Cell::default(); self.cols as usize]
    }

    pub(super) fn scroll_up(&mut self, n: u16) {
        for _ in 0..n {
            self.grid.remove(0);
            self.grid.push(self.blank_row());
        }
    }

    pub(super) fn scroll_down(&mut self, n: u16) {
        for _ in 0..n {
            self.grid.insert(0, self.blank_row());
            self.grid.pop();
        }
    }

    pub(super) fn linefeed(&mut self) {
        let (r, _) = self.cursor;
        if r + 1 >= self.rows {
            self.scroll_up(1);
            self.cursor.0 = self.rows - 1;
        } else {
            self.cursor.0 = r + 1;
        }
    }

    pub(super) fn put_char(&mut self, c: char) {
        let (r, cidx) = self.cursor;
        let style = effective_style_of(&self.style);
        self.grid[r as usize][cidx as usize] = Cell { ch: c, style };
        self.cursor.1 += 1;
        if self.cursor.1 >= self.cols {
            self.cursor.1 = 0;
            self.linefeed();
        }
    }

    pub(super) fn erase_in_line(&mut self, mode: u16) {
        let (r, c) = self.cursor;
        let row = &mut self.grid[r as usize];
        let blank = Cell::default();
        match mode {
            0 => for i in c as usize..row.len() { row[i] = blank.clone(); },
            1 => for i in 0..=c.min(self.cols - 1) as usize { row[i] = blank.clone(); },
            _ => for i in 0..row.len() { row[i] = blank.clone(); },
        }
    }

    pub(super) fn erase_in_display(&mut self, mode: u16) {
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

    pub(super) fn apply_sgr(&mut self, codes: &[u16]) {
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
