//! 轮转日志文件封装（file-rotate）：达到 max_bytes 轮转，旧分块 `.1 .2 …` 计数后缀，
//! 保留 max_files 份，可选 gzip 压缩（FR-4.3 / CONFIG-SPEC stdout_logfile_*）。

use crate::events::LogLine;
use file_rotate::compression::Compression;
use file_rotate::suffix::AppendCount;
use file_rotate::{ContentLimit, FileRotate};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct RotatingFile {
    inner: FileRotate<AppendCount>,
    path: PathBuf,
}

impl RotatingFile {
    /// `max_bytes == 0` 表示不轮转（CONFIG-SPEC stdout_logfile_maxbytes=0 语义）。
    pub fn new(path: &Path, max_bytes: u64, max_files: u32, compress: bool) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建日志目录失败：{e}"))?;
        }
        let compression = if compress {
            Compression::OnRotate(2)
        } else {
            Compression::None
        };
        let limit = if max_bytes == 0 {
            ContentLimit::Bytes(usize::MAX)
        } else {
            ContentLimit::Bytes(max_bytes as usize)
        };
        let inner = FileRotate::new(
            path.to_path_buf(),
            AppendCount::new(max_files.max(1) as usize),
            limit,
            compression,
            None,
        );
        Ok(Self { inner, path: path.to_path_buf() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 追加一行（含换行）。
    pub fn write_line(&mut self, line: &LogLine) -> std::io::Result<()> {
        writeln!(self.inner, "{}", line.text)
    }
}
