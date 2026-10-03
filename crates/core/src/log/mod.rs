//! 日志管道（HLD §3.3）：行解码 → 环形缓冲 + 可选轮转文件（批量广播由 service 层 hub 完成）。
//!
//! - 环形缓冲每服务默认保留最近 2000 行（可配），超出丢弃最旧行（FR-4.2）；
//! - 文件写入即落盘，按 max_bytes 轮转 `.1 .2 …`，保留 max_files 份，可选 gzip（FR-4.3）；
//! - 单行超 64KB 截断（NFR-6）；UTF-8 lossy 解码（FR-4.5）。

mod rotate;

use crate::events::{LogLine, Stream};
use chrono::Local;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub use rotate::RotatingFile;

/// 单行长度上限（NFR-6）：超出截断，防极端输入
pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// 当前本地时间戳字符串
pub fn now_ts() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// 每服务日志仓库：环形缓冲（必选）+ 轮转文件（可选）。
pub struct LogStore {
    ring: Mutex<VecDeque<LogLine>>,
    next_seq: AtomicU64,
    cap: AtomicU64,
    file: Mutex<Option<RotatingFile>>,
}

impl LogStore {
    pub fn new(
        cap: usize,
        file_policy: Option<&crate::config::LogFilePolicy>,
    ) -> Result<Self, String> {
        let file = match file_policy {
            Some(f) => Some(RotatingFile::new(&f.path, f.max_bytes, f.max_files, f.compress)?),
            None => None,
        };
        Ok(Self {
            ring: Mutex::new(VecDeque::new()),
            next_seq: AtomicU64::new(0),
            cap: AtomicU64::new(cap as u64),
            file: Mutex::new(file),
        })
    }

    pub fn set_cap(&self, cap: usize) {
        self.cap.store(cap as u64, Ordering::Relaxed);
        self.trim();
    }

    /// 替换文件 sink（配置热更后即时生效；失败时保留旧 sink 并返回错误）
    pub fn replace_file(
        &self,
        policy: Option<&crate::config::LogFilePolicy>,
    ) -> Result<(), String> {
        let new_file = match policy {
            Some(f) => Some(RotatingFile::new(&f.path, f.max_bytes, f.max_files, f.compress)?),
            None => None,
        };
        *self.file.lock().unwrap() = new_file;
        Ok(())
    }

    fn trim(&self) {
        let cap = self.cap.load(Ordering::Relaxed) as usize;
        let mut ring = self.ring.lock().unwrap();
        while ring.len() > cap {
            ring.pop_front();
        }
    }

    /// 追加一行：分配序号与时间戳，写环形缓冲与文件（如有）。
    /// 超长行在存储层兜底截断（NFR-6；LineDecoder 已先行截断）。
    pub fn push(&self, stream: Stream, text: &str) -> LogLine {
        let owned;
        let text = if text.chars().count() > MAX_LINE_BYTES {
            owned = format!(
                "{} …[行超 64KB 已截断]",
                text.chars().take(MAX_LINE_BYTES).collect::<String>()
            );
            &owned
        } else {
            text
        };
        let line = LogLine {
            seq: self.next_seq.fetch_add(1, Ordering::Relaxed) + 1,
            ts: now_ts(),
            stream,
            text: text.to_string(),
        };
        {
            let mut ring = self.ring.lock().unwrap();
            ring.push_back(line.clone());
        }
        self.trim();
        if let Some(f) = self.file.lock().unwrap().as_mut() {
            // 写入即落盘要求（FR-4.3）；写失败不影响内存缓冲，只记 warn
            if let Err(e) = f.write_line(&line) {
                log::warn!("日志文件写入失败：{e}");
            }
        }
        line
    }

    /// 历史回放：seq > since 的最早 limit 条（HLD §4.1 get_log）。
    pub fn snapshot(&self, since: u64, limit: usize) -> Vec<LogLine> {
        let ring = self.ring.lock().unwrap();
        ring.iter()
            .filter(|l| l.seq > since)
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn last_seq(&self) -> u64 {
        self.next_seq.load(Ordering::Relaxed)
    }

    pub fn len(&self) -> usize {
        self.ring.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 日志文件路径（「打开日志目录」用）
    pub fn file_path(&self) -> Option<PathBuf> {
        self.file.lock().unwrap().as_ref().map(|f| f.path().to_path_buf())
    }
}

use std::path::PathBuf;

/// 增量字节流 → 行解码器。
///
/// - 按 `\n` 切行；`strip_cr=true` 时去掉行尾 `\r`（PTY 输出为 CRLF）；
/// - 超过 [`MAX_LINE_BYTES`] 的行截断（NFR-6）；
/// - 非法 UTF-8 字节以替换符显示（FR-4.5）；
/// - 流结束（EOF）时若还有残余行，由 [`LineDecoder::finish`] 产出。
#[derive(Default)]
pub struct LineDecoder {
    buf: Vec<u8>,
    truncated: bool,
}

impl LineDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一段字节，返回其中完整的行。
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &b in chunk {
            if b == b'\n' {
                lines.push(self.take_line());
            } else {
                if self.buf.len() >= MAX_LINE_BYTES {
                    if !self.truncated {
                        self.truncated = true;
                    }
                    continue;
                }
                self.buf.push(b);
            }
        }
        lines
    }

    /// 流结束时调用：产出残余的最后一行（可能被截断）。
    pub fn finish(&mut self) -> Option<String> {
        if self.buf.is_empty() && !self.truncated {
            return None;
        }
        Some(self.take_line())
    }

    fn take_line(&mut self) -> String {
        let truncated = std::mem::take(&mut self.truncated);
        let mut buf = std::mem::take(&mut self.buf);
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        let mut s = String::from_utf8_lossy(&buf).into_owned();
        if truncated {
            s.push_str(" …[行超 64KB 已截断]");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LogFilePolicy;

    fn store(cap: usize) -> LogStore {
        LogStore::new(cap, None).unwrap()
    }

    #[test]
    fn ring_buffer_evicts_oldest() {
        let s = store(3);
        for i in 1..=5 {
            s.push(Stream::Out, &format!("line{i}"));
        }
        let all = s.snapshot(0, 100);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].text, "line3");
        assert_eq!(all[2].text, "line5");
        assert_eq!(all[2].seq, 5, "seq 单调递增不因淘汰重置");
    }

    #[test]
    fn snapshot_since_limit() {
        let s = store(100);
        for i in 1..=10 {
            s.push(Stream::Out, &format!("l{i}"));
        }
        let part = s.snapshot(7, 2);
        assert_eq!(part.len(), 2);
        assert_eq!(part[0].text, "l8");
        assert_eq!(part[1].text, "l9");
    }

    #[test]
    fn set_cap_shrinks() {
        let s = store(100);
        for i in 0..50 {
            s.push(Stream::Out, &format!("l{i}"));
        }
        s.set_cap(10);
        assert_eq!(s.len(), 10);
    }

    #[test]
    fn long_line_truncated() {
        let s = store(10);
        let big = "x".repeat(MAX_LINE_BYTES + 100);
        let line = s.push(Stream::Err, &big);
        assert!(line.text.len() < MAX_LINE_BYTES + 100);
        assert!(line.text.contains("已截断"));
    }

    #[test]
    fn file_sink_rotates_and_compresses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("svc.log");
        let policy = LogFilePolicy {
            path: path.clone(),
            max_bytes: 200,
            max_files: 3,
            compress: true,
        };
        let s = LogStore::new(1000, Some(&policy)).unwrap();
        for i in 0..100 {
            s.push(Stream::Out, &format!("log line number {i:04} padding padding"));
        }
        // 主文件 + 若干分块；分块中存在 .gz
        let entries: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(entries.iter().any(|n| n.ends_with(".gz")), "{entries:?}");
        assert!(entries.len() <= 6, "保留份数受限：{entries:?}");
    }

    #[test]
    fn line_decoder_basic() {
        let mut d = LineDecoder::new();
        let lines = d.feed(b"hello\r\nwor");
        assert_eq!(lines, vec!["hello"]);
        let lines = d.feed("ld\n中文\n".as_bytes());
        assert_eq!(lines, vec!["world", "中文"]);
        assert_eq!(d.finish(), None);
    }

    #[test]
    fn line_decoder_truncates_overlong_line() {
        let mut d = LineDecoder::new();
        let big = vec![b'a'; MAX_LINE_BYTES + 50];
        let lines = d.feed(&big);
        assert!(lines.is_empty());
        let lines = d.feed(b"\nnext\n");
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("已截断"));
        assert_eq!(lines[1], "next");
    }

    #[test]
    fn line_decoder_finish_flushes_partial() {
        let mut d = LineDecoder::new();
        d.feed(b"partial line");
        assert_eq!(d.finish().as_deref(), Some("partial line"));
        assert_eq!(d.finish(), None);
    }

    #[test]
    fn line_decoder_split_utf8_across_feeds() {
        let mut d = LineDecoder::new();
        let s = "你好\n".as_bytes().to_vec();
        let (a, b) = s.split_at(4); // 切在多字节中间
        assert!(d.feed(a).is_empty());
        let lines = d.feed(b);
        assert_eq!(lines, vec!["你好"]);
    }
}
