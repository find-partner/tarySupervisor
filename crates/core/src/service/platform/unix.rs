//! Unix spawn 实现：setsid 进程组 + killpg 优雅/强杀；PTY 走 portable-pty。

use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tokio::sync::oneshot;

use super::{default_cwd, resolve_command, spawn_pty_reader_thread, spawn_reader_thread, ExitInfo, Killer, RunCtx};
use super::HubMsg;
use crate::config::ServiceConfig;

fn signal_killer(pid: u32, sig: libc::c_int) -> Killer {
    Killer::new(
        move || unsafe {
            let p = pid as libc::pid_t;
            // 仅当子进程自成进程组（setsid 生效）时 killpg，避免误伤自身进程组
            if libc::getpgid(p) == p {
                libc::killpg(p, sig);
            } else {
                libc::kill(p, sig);
            }
        },
        move || unsafe {
            let p = pid as libc::pid_t;
            if libc::getpgid(p) == p {
                libc::killpg(p, libc::SIGKILL);
            } else {
                libc::kill(p, libc::SIGKILL);
            }
        },
    )
}

pub(super) fn spawn_pipe(
    cfg: &ServiceConfig,
    env: &std::collections::BTreeMap<String, String>,
    hub_tx: &tokio::sync::mpsc::UnboundedSender<HubMsg>,
) -> Result<RunCtx, String> {
    use std::os::unix::process::CommandExt;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};

    let (prog, args) = resolve_command(cfg);
    let mut cmd = Command::new(&prog);
    cmd.args(&args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.current_dir(default_cwd(cfg));
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    // redirect_stderr 用「流标识合并」实现（两根管道，stderr 行按 Out 标记），效果等同单流
    cmd.stderr(Stdio::piped());
    // FR-3.2：setsid 新会话，子进程自成进程组
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("无法启动 {}：{e}", cfg.command))?;
    let pid = child.id();
    let out = child.stdout.take();
    let err = child.stderr.take();
    if let Some(out) = out {
        spawn_reader_thread(Box::new(out), crate::events::Stream::Out, false, hub_tx.clone());
    }
    if let Some(err) = err {
        spawn_reader_thread(
            Box::new(err),
            crate::events::Stream::Err,
            cfg.redirect_stderr,
            hub_tx.clone(),
        );
    }
    let (exit_tx, exit_rx) = oneshot::channel();
    std::thread::spawn(move || {
        let info = match child.wait() {
            Ok(st) => ExitInfo { code: st.code(), signal: st.signal() },
            Err(e) => {
                log::warn!("wait 失败：{e}");
                ExitInfo { code: Some(-1), signal: None }
            }
        };
        let _ = exit_tx.send(info);
    });
    Ok(RunCtx { pid, killer: signal_killer(pid, libc::SIGTERM), exit: exit_rx, pty_writer: None })
}

pub(super) fn spawn_pty(
    cfg: &ServiceConfig,
    env: &std::collections::BTreeMap<String, String>,
    term: Option<&Arc<super::TermHandle>>,
    hub_tx: &tokio::sync::mpsc::UnboundedSender<HubMsg>,
) -> Result<RunCtx, String> {
    let term = term.cloned().ok_or("PTY 模式缺少终端句柄")?;
    let pair = native_pty_system()
        .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("打开 PTY 失败：{e}"))?;
    let (prog, args) = resolve_command(cfg);
    let mut cmd = CommandBuilder::new(&prog);
    cmd.args(args.iter());
    cmd.cwd(default_cwd(cfg));
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("无法启动 {}：{e}", cfg.command))?;
    let pid = child.process_id().ok_or("PTY 子进程无 PID")?;
    // 父进程侧释放 slave，使 master 读到 EOF
    drop(pair.slave);
    let reader = pair.master.try_clone_reader().map_err(|e| format!("PTY reader 失败：{e}"))?;
    let writer = pair.master.take_writer().map_err(|e| format!("PTY writer 失败：{e}"))?;
    spawn_pty_reader_thread(Box::new(reader), term, hub_tx.clone());

    let soft = child.clone_killer();
    let soft = Arc::new(Mutex::new(soft));
    let soft2 = soft.clone();
    let killer = Killer::new(
        move || unsafe {
            let p = pid as libc::pid_t;
            if libc::getpgid(p) == p {
                libc::killpg(p, libc::SIGTERM);
            } else {
                let _ = soft2.lock().unwrap().kill();
            }
        },
        move || unsafe {
            let p = pid as libc::pid_t;
            if libc::getpgid(p) == p {
                libc::killpg(p, libc::SIGKILL);
            } else {
                let _ = soft.lock().unwrap().kill();
            }
        },
    );

    // portable-pty 的 ExitStatus 不含信号信息：自行 waitpid（它不会在我们不调用 wait() 时回收）
    let (exit_tx, exit_rx) = oneshot::channel();
    std::thread::spawn(move || {
        let mut status: libc::c_int = 0;
        let info;
        loop {
            let r = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) };
            if r == pid as libc::pid_t {
                if libc::WIFSIGNALED(status) {
                    info = ExitInfo { code: None, signal: Some(libc::WTERMSIG(status)) };
                } else if libc::WIFEXITED(status) {
                    info = ExitInfo { code: Some(libc::WEXITSTATUS(status)), signal: None };
                } else {
                    info = ExitInfo { code: Some(-1), signal: None };
                }
                break;
            }
            if r < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                info = ExitInfo { code: Some(-1), signal: None };
                break;
            }
        }
        let _ = exit_tx.send(info);
    });
    Ok(RunCtx { pid, killer, exit: exit_rx, pty_writer: Some(writer) })
}
