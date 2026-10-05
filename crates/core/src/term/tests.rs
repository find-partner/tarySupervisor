//! 终端仿真测试：换行/回车重绘、SGR 颜色、光标定位、滚动、反显、TAB/BS。

use vte::Parser;

use super::emu::TermEmulator;
use super::{Color, Style, TermHandle};

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
