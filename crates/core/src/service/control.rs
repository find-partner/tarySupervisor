//! ServiceManager 启停 / 查询 / 终端输入 API（类型与装配见 super）。

use std::io::Write;
use std::time::{Duration, Instant};

use super::{ServiceBrief, ServiceManager, SvcCmd};
use crate::events::{LogLine, MatchItem, SvcState};
use crate::term::TermScreen;

impl ServiceManager {
    pub fn list(&self) -> Vec<ServiceBrief> {
        let configs = self.shared.configs.read().unwrap().clone();
        configs
            .iter()
            .map(|cfg| {
                let sup = self.shared.sups.read().unwrap().get(&cfg.id).cloned();
                let (state, pid, exit_code, reason, since_ms) = match sup {
                    Some(s) => {
                        let i = s.state_rx.borrow().clone();
                        (i.state, i.pid, i.exit_code, i.reason, i.since_ms)
                    }
                    None => (SvcState::Stopped, None, None, None, 0),
                };
                let match_items = self.match_items(&cfg.id);
                ServiceBrief {
                    config: cfg.clone(),
                    state,
                    pid,
                    exit_code,
                    reason,
                    since_ms,
                    match_items,
                }
            })
            .collect()
    }

    fn send_cmd(&self, id: &str, cmd: SvcCmd) -> Result<(), String> {
        let sups = self.shared.sups.read().unwrap();
        let sup = sups.get(id).ok_or_else(|| format!("服务 {id} 不存在"))?;
        sup.cmd_tx
            .send(cmd)
            .map_err(|_| format!("服务 {id} 的监督任务已退出"))?;
        Ok(())
    }

    fn state_of(&self, id: &str) -> Option<SvcState> {
        let sups = self.shared.sups.read().unwrap();
        sups.get(id).map(|s| s.state_rx.borrow().state)
    }

    pub fn start(&self, id: &str) -> Result<(), String> {
        match self.state_of(id) {
            None => Err(format!("服务 {id} 不存在")),
            Some(st) if matches!(st, SvcState::Stopped | SvcState::Exited | SvcState::Fatal | SvcState::Backoff) => {
                self.send_cmd(id, SvcCmd::Start)
            }
            Some(st) => Err(format!("服务 {id} 当前为 {}，无需启动", st.as_str())),
        }
    }

    pub fn stop(&self, id: &str) -> Result<(), String> {
        match self.state_of(id) {
            None => Err(format!("服务 {id} 不存在")),
            Some(st) if matches!(st, SvcState::Starting | SvcState::Running | SvcState::Stopping | SvcState::Backoff) => {
                self.send_cmd(id, SvcCmd::Stop)
            }
            Some(st) => Err(format!("服务 {id} 当前为 {}，无需停止", st.as_str())),
        }
    }

    pub fn restart(&self, id: &str) -> Result<(), String> {
        match self.state_of(id) {
            None => Err(format!("服务 {id} 不存在")),
            Some(SvcState::Stopping) => Err(format!("服务 {id} 正在停止，请稍后重试")),
            Some(_) => self.send_cmd(id, SvcCmd::Restart),
        }
    }

    pub fn get_log(&self, id: &str, since: u64, limit: usize) -> Option<Vec<LogLine>> {
        let ctxs = self.shared.ctxs.read().unwrap();
        ctxs.get(id).map(|c| c.store.snapshot(since, limit))
    }

    pub fn match_items(&self, id: &str) -> Vec<MatchItem> {
        let ctxs = self.shared.ctxs.read().unwrap();
        match ctxs.get(id) {
            Some(c) => c.matcher.lock().unwrap().items(),
            None => vec![],
        }
    }

    /// 按哈希查找快捷项 URL（托盘 match:<id>:<hash> 事件路由，FR-1.7 / FR-5.6）
    pub fn match_url(&self, id: &str, hash: u64) -> Option<String> {
        let ctx = self.shared.ctxs.read().unwrap().get(id).cloned();
        let Some(ctx) = ctx else { return None };
        let items = ctx.matcher.lock().unwrap().items();
        items.into_iter().find(|i| i.hash == hash).and_then(|i| i.url)
    }

    /// 只读终端快照（FR-6.2：视图只是输出订阅者，PTY master 由后端持有）
    pub fn term_snapshot(&self, id: &str) -> Option<TermScreen> {
        let ctx = self.shared.ctxs.read().unwrap().get(id).cloned();
        let Some(ctx) = ctx else { return None };
        Some(ctx.term.screen())
    }

    /// 转发输入至服务 PTY master（SRS FR-6.4）。
    /// 仅 `term_interactive` + Pty 模式且进程运行中可用；Pipe 模式 stdin 已关闭。
    pub fn write_stdin(&self, id: &str, data: Vec<u8>) -> Result<(), String> {
        let ctx = self
            .shared
            .ctxs
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| format!("服务 {id} 不存在或尚未加载"))?;
        let mut guard = ctx.pty_writer.lock().unwrap();
        match guard.as_mut() {
            Some(w) => w
                .write_all(&data)
                .and_then(|_| w.flush())
                .map_err(|e| format!("写入服务 {id} 的 stdin 失败：{e}")),
            None => Err(format!(
                "服务 {id} 未运行或非 PTY 模式，无法接收输入"
            )),
        }
    }

    /// 运行中服务数（托盘 tooltip：运行中 n / 共 m 个服务）
    pub fn running_count(&self) -> usize {
        let sups = self.shared.sups.read().unwrap();
        sups.values().filter(|s| s.state_rx.borrow().state.is_alive()).count()
    }

    pub fn total_count(&self) -> usize {
        self.shared.sups.read().unwrap().len()
    }

    /// 应用退出：对全部运行中服务执行停止流程并等待完成（FR-3.7）。
    /// 阻塞直至全部停止（上限 = 最大 stop_timeout + 10s）。
    pub fn shutdown_all(&self) {
        {
            let sups = self.shared.sups.read().unwrap();
            for sup in sups.values() {
                let _ = sup.cmd_tx.send(SvcCmd::Shutdown);
            }
            let max_timeout = self
                .shared
                .configs
                .read()
                .unwrap()
                .iter()
                .map(|s| s.stop_timeout_secs)
                .max()
                .unwrap_or(10);
            let deadline = Instant::now() + Duration::from_secs(max_timeout + 10);
            loop {
                let alive = sups
                    .values()
                    .filter(|s| s.state_rx.borrow().state.is_alive())
                    .count();
                if alive == 0 || Instant::now() >= deadline {
                    if alive > 0 {
                        log::warn!("退出等待超时，仍有 {alive} 个服务未完全停止");
                    }
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}
