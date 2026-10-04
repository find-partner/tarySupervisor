//! 每服务监督任务：状态机（FR-3.1）+ 启动/停止/重启流程（FR-3.2 ~ FR-3.5）。
//!
//! ```text
//! STOPPED ──start──► STARTING ──startsecs 或首行输出──► RUNNING
//!    ▲                  │失败/意外退出                     │
//!    │                  ▼                                 ▼ stop
//!    └── 拉起失败 ◄── BACKOFF(1s,2s,4s…≤startretries) ◄─ STOPPING ──优雅→超时强杀──► STOPPED
//!                         │ 重试耗尽
//!                         ▼
//!                       FATAL ── 手动 start 可复活
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tokio::time::{sleep_until, Instant as TInstant};

use super::platform::{spawn_service, ExitInfo, RunCtx};
use super::{HubMsg, SvcCmd, SvcCtx};
use crate::config::{Autorestart, ServiceConfig};
use crate::events::{CoreEvent, EventBus, StateInfo, StopReason, SvcState};
use crate::matcher::MatcherState;
use crate::term::TermHandle;

pub(super) struct SupervisorDeps {
    pub id: String,
    pub bus: EventBus,
    pub cfg_rx: watch::Receiver<ServiceConfig>,
    pub state_tx: watch::Sender<StateInfo>,
    pub hub_tx: mpsc::UnboundedSender<HubMsg>,
    pub got_output: Arc<AtomicBool>,
    pub matcher: Arc<Mutex<MatcherState>>,
    pub term: Arc<TermHandle>,
    /// 运行态上下文（pty_writer 挂靠，供 write_stdin 转发输入，FR-6.4）
    pub ctx: Arc<SvcCtx>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn exit_reason(signal: Option<i32>, forced: bool) -> StopReason {
    if let Some(sig) = signal {
        StopReason::Signal(sig)
    } else if forced {
        StopReason::ForcedTimeout
    } else {
        StopReason::Normal
    }
}

fn backoff_delay(retries: u32) -> Duration {
    Duration::from_secs(1u64 << (retries - 1).min(4))
}

/// 意外退出信息（叠加强杀标记）
struct Died {
    code: Option<i32>,
    signal: Option<i32>,
    forced: bool,
}

impl Died {
    fn is_success(&self) -> bool {
        self.code == Some(0) && self.signal.is_none() && !self.forced
    }
}

/// run_phase 结束后的去向
enum Outcome {
    Stopped(Option<i32>, StopReason),
    /// 手动重启：停止完成后立即重新 spawn（FR-3.4）
    Restart(Option<i32>, StopReason),
    /// Shutdown：停止完成后任务退出
    Shutdown(Option<i32>, StopReason),
    Died(Died),
}

pub(super) async fn supervise(d: SupervisorDeps, mut cmd_rx: mpsc::UnboundedReceiver<SvcCmd>) {
    let mut state = SvcState::Stopped;
    let mut retries: u32 = 0;
    let mut backoff_until = TInstant::now();
    let mut info =
        StateInfo { state, pid: None, exit_code: None, reason: None, since_ms: now_ms() };
    let _ = d.state_tx.send(info.clone());

    // 状态迁移同时更新 watch（供查询）与广播（供托盘/前端）
    fn set_state(
        d: &SupervisorDeps,
        info: &mut StateInfo,
        state: SvcState,
        pid: Option<u32>,
        exit_code: Option<i32>,
        reason: Option<StopReason>,
    ) {
        info.state = state;
        info.pid = pid;
        info.exit_code = exit_code;
        info.reason = reason.clone();
        info.since_ms = now_ms();
        let _ = d.state_tx.send(info.clone());
        d.bus.send(CoreEvent::StateChanged { id: d.id.clone(), info: info.clone() });
    }

    fn sys_line(d: &SupervisorDeps, text: String) {
        let _ = d.hub_tx.send(HubMsg::System(text));
    }

    'outer: loop {
        // ---------- 空闲态：Stopped / Exited / Fatal / Backoff ----------
        match state {
            SvcState::Stopped | SvcState::Exited | SvcState::Fatal => {
                match cmd_rx.recv().await {
                    Some(SvcCmd::Start) | Some(SvcCmd::Restart) => {
                        retries = 0;
                        info.reason = None;
                    }
                    Some(SvcCmd::Stop) => continue,
                    Some(SvcCmd::Shutdown) | None => break 'outer,
                }
            }
            SvcState::Backoff => {
                tokio::select! {
                    _ = sleep_until(backoff_until) => {}
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(SvcCmd::Start) | Some(SvcCmd::Restart) => { retries = 0; }
                            Some(SvcCmd::Stop) => {
                                set_state(&d, &mut info, SvcState::Stopped, None, None, None);
                                state = SvcState::Stopped;
                                continue;
                            }
                            Some(SvcCmd::Shutdown) | None => {
                                set_state(&d, &mut info, SvcState::Stopped, None, None, None);
                                break 'outer;
                            }
                        }
                    }
                }
            }
            _ => unreachable!("运行态在 run_phase 内消化"),
        }

        // ---------- 启动循环（重启 = 停止完成后回到这里重新 spawn） ----------
        loop {
            let cfg = d.cfg_rx.borrow().clone();
            // 服务重启后清空该服务的 Matcher 快捷项（FR-5.5）
            d.matcher.lock().unwrap().clear();
            d.bus.send(CoreEvent::MatcherHit { svc_id: d.id.clone(), items: vec![] });
            d.got_output.store(false, Ordering::Relaxed);
            // 清空上一轮的 PTY 写端（未运行期间不可写，FR-6.4）
            *d.ctx.pty_writer.lock().unwrap() = None;
            set_state(&d, &mut info, SvcState::Starting, None, None, None);

            match spawn_service(&cfg, &resolve_env(&cfg), Some(&d.term), &d.hub_tx) {
                Err(e) => {
                    sys_line(&d, format!("[traysv] 启动失败：{e}"));
                    retries += 1;
                    if retries > cfg.startretries {
                        sys_line(&d, format!(
                            "[traysv] 状态：FATAL（启动重试耗尽，startretries={}）",
                            cfg.startretries
                        ));
                        set_state(&d, &mut info, SvcState::Fatal, None, None,
                            Some(StopReason::SpawnError(e)));
                        state = SvcState::Fatal;
                        break;
                    }
                    let delay = backoff_delay(retries);
                    sys_line(&d, format!(
                        "[traysv] 状态：BACKOFF（{}s 后第 {retries} 次重试）",
                        delay.as_secs()
                    ));
                    set_state(&d, &mut info, SvcState::Backoff, None, None,
                        Some(StopReason::SpawnError(e)));
                    backoff_until = TInstant::now() + delay;
                    state = SvcState::Backoff;
                    break;
                }
                Ok(mut run) => {
                    let pid = run.pid;
                    // PTY 写端挂到 ctx：运行期间 write_stdin 可用（SRS FR-6.4）
                    *d.ctx.pty_writer.lock().unwrap() = run.pty_writer.take();
                    set_state(&d, &mut info, SvcState::Starting, Some(pid), None, None);
                    sys_line(&d, format!("[traysv] 已启动（PID {pid}）"));
                    let outcome =
                        run_phase(&d, &mut info, &cfg, run, &mut cmd_rx, &mut state, pid).await;
                    // 退出后写端失效
                    *d.ctx.pty_writer.lock().unwrap() = None;
                    match outcome {
                        Outcome::Stopped(ecode, reason) => {
                            set_state(&d, &mut info, SvcState::Stopped, None, ecode, Some(reason));
                            state = SvcState::Stopped;
                            break;
                        }
                        Outcome::Restart(ecode, reason) => {
                            set_state(&d, &mut info, SvcState::Stopped, None, ecode, Some(reason));
                            // 立即重新启动（内层循环继续）
                        }
                        Outcome::Shutdown(ecode, reason) => {
                            set_state(&d, &mut info, SvcState::Stopped, None, ecode, Some(reason));
                            break 'outer;
                        }
                        Outcome::Died(exit) => {
                            let reason = exit_reason(exit.signal, exit.forced);
                            match (exit.signal, exit.code) {
                                (Some(sig), _) => {
                                    sys_line(&d, format!("[traysv] 进程被信号 {sig} 终止"))
                                }
                                (None, Some(c)) => sys_line(&d, format!(
                                    "[traysv] 进程退出：exit code={c}（{}）",
                                    reason.describe()
                                )),
                                _ => sys_line(&d, "[traysv] 进程退出".to_string()),
                            }
                            let mut next = SvcState::Exited;
                            match cfg.autorestart {
                                Autorestart::No => {}
                                Autorestart::OnFailure if exit.is_success() => {}
                                _ => {
                                    retries += 1;
                                    if retries > cfg.startretries {
                                        next = SvcState::Fatal;
                                        sys_line(&d,
                                            "[traysv] 状态：FATAL（自动重启重试耗尽）".to_string());
                                    } else {
                                        next = SvcState::Backoff;
                                        let delay = backoff_delay(retries);
                                        sys_line(&d, format!(
                                            "[traysv] 状态：BACKOFF（{}s 后第 {retries} 次自动重启）",
                                            delay.as_secs()
                                        ));
                                        backoff_until = TInstant::now() + delay;
                                    }
                                }
                            }
                            set_state(&d, &mut info, next, None, exit.code, Some(reason));
                            state = next;
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// 运行阶段：STARTING → RUNNING、处理停止/重启命令、等待退出（优雅→超时强杀）。
#[allow(clippy::too_many_arguments)]
async fn run_phase(
    d: &SupervisorDeps,
    info: &mut StateInfo,
    cfg: &ServiceConfig,
    run: RunCtx,
    cmd_rx: &mut mpsc::UnboundedReceiver<SvcCmd>,
    state: &mut SvcState,
    pid: u32,
) -> Outcome {
    let mut exit = run.exit;
    let killer = run.killer;

    let startsecs = Duration::from_secs(cfg.startsecs);
    let started_at = Instant::now();
    let mut announced_running = false;

    let mut stopping = false;
    let mut forced = false;
    let mut want_restart = false;
    let mut want_shutdown = false;
    let mut stop_deadline: TInstant = TInstant::now();
    let mut hard_deadline: TInstant = TInstant::now();

    loop {
        tokio::select! {
            res = &mut exit => {
                let e = res.unwrap_or(ExitInfo { code: Some(-1), signal: None });
                let reason = exit_reason(e.signal, forced);
                return if stopping {
                    if want_shutdown {
                        Outcome::Shutdown(e.code, reason)
                    } else if want_restart {
                        Outcome::Restart(e.code, reason)
                    } else {
                        Outcome::Stopped(e.code, reason)
                    }
                } else {
                    Outcome::Died(Died { code: e.code, signal: e.signal, forced })
                };
            }

            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SvcCmd::Start) => {} // 运行中忽略
                    Some(SvcCmd::Stop) | None => begin_stop(
                        cfg, &killer, d, info, state, pid,
                        &mut stopping, &mut want_restart, &mut want_shutdown,
                        &mut stop_deadline, &mut hard_deadline, false, false,
                    ),
                    Some(SvcCmd::Restart) => begin_stop(
                        cfg, &killer, d, info, state, pid,
                        &mut stopping, &mut want_restart, &mut want_shutdown,
                        &mut stop_deadline, &mut hard_deadline, true, false,
                    ),
                    Some(SvcCmd::Shutdown) => begin_stop(
                        cfg, &killer, d, info, state, pid,
                        &mut stopping, &mut want_restart, &mut want_shutdown,
                        &mut stop_deadline, &mut hard_deadline, false, true,
                    ),
                }
            }

            // STARTING → RUNNING：存活超过 startsecs 或首行输出（50ms 轮询，无唤醒竞态）
            _ = tokio::time::sleep(Duration::from_millis(50)),
                if !announced_running && !stopping =>
            {
                if d.got_output.load(Ordering::Relaxed) || started_at.elapsed() >= startsecs {
                    announced_running = true;
                    emit(d, info, state, SvcState::Running, Some(pid));
                }
            }

            // 优雅停止超时 → 强杀整树（FR-3.3）
            _ = sleep_until(stop_deadline), if stopping && !forced => {
                killer.force();
                forced = true;
            }

            // 强杀后仍未回收（异常情况）：放弃等待，按停止完成处理
            _ = sleep_until(hard_deadline), if stopping && forced => {
                let reason = StopReason::ForcedTimeout;
                return if want_shutdown {
                    Outcome::Shutdown(None, reason)
                } else if want_restart {
                    Outcome::Restart(None, reason)
                } else {
                    Outcome::Stopped(None, reason)
                };
            }
        }
    }
}

/// 状态迁移：更新 watch 投影并广播 StateChanged
fn emit(
    d: &SupervisorDeps,
    info: &mut StateInfo,
    state: &mut SvcState,
    st: SvcState,
    pid: Option<u32>,
) {
    *state = st;
    info.state = st;
    if pid.is_some() {
        info.pid = pid;
    }
    info.since_ms = now_ms();
    let _ = d.state_tx.send(info.clone());
    d.bus.send(CoreEvent::StateChanged { id: d.id.clone(), info: info.clone() });
}

/// 进入 STOPPING：发优雅信号、起超时计时（FR-3.3）
#[allow(clippy::too_many_arguments)]
fn begin_stop(
    cfg: &ServiceConfig,
    killer: &super::platform::Killer,
    d: &SupervisorDeps,
    info: &mut StateInfo,
    state: &mut SvcState,
    pid: u32,
    stopping: &mut bool,
    want_restart: &mut bool,
    want_shutdown: &mut bool,
    stop_deadline: &mut TInstant,
    hard_deadline: &mut TInstant,
    is_restart: bool,
    is_shutdown: bool,
) {
    if !*stopping {
        *stopping = true;
        *want_restart = is_restart;
        *want_shutdown = is_shutdown;
        let timeout = Duration::from_secs(cfg.stop_timeout_secs);
        *stop_deadline = TInstant::now() + timeout;
        *hard_deadline = *stop_deadline + Duration::from_secs(10);
        emit(d, info, state, SvcState::Stopping, Some(pid));
        killer.graceful();
    } else if is_shutdown {
        *want_shutdown = true;
        *want_restart = false;
    }
}

fn resolve_env(cfg: &ServiceConfig) -> std::collections::BTreeMap<String, String> {
    crate::config::resolve_env(&cfg.env, &crate::config::system_var).unwrap_or_default()
}
