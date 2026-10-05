//! vte Perform 回调：把解析出的打印字符 / 控制字节 / CSI / ESC 序列映射到状态机。

use vte::Perform;

use super::emu::TermEmulator;
use super::{Cell, Style};

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
