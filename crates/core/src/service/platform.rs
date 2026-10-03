//! 进程 spawn/stop 平台分支（HLD §1.2 原则 4：平台差异集中于此）。
//!
//! - 管道模式：
//!   - Unix：`pre_exec(setsid)` 新会话（进程组 = 子进程），killpg 优雅/强杀；
//!   - Windows：`CreateProcessW(CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP)` →
//!     `AssignProcessToJobObject(KILL_ON_JOB_CLOSE)` → `ResumeThread`
//!     （spike `windows/job-object-kill-tree` 已验证的时序，保证子进程无逃逸窗口）；
//!     优雅停止尽力尝试 `CTRL_BREAK_EVENT`，失败则回退终止直接子进程；强杀 `TerminateJobObject`。
//! - PTY 模式：portable-pty（Unix openpty+setsid / Windows ConPTY）。
//!
//! FR-3.6：全程不经 shell 包装，参数按数组直接传给目标程序。

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write}; // Write 供 RunCtx.pty_writer（FR-6.4 预留输入通道）
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use super::HubMsg;
use crate::config::ServiceConfig;

/// 退出信息：code 为 None 表示被信号终止（Unix）
#[derive(Debug, Clone)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

impl ExitInfo {
    pub fn is_success(&self) -> bool {
        self.code == Some(0) && self.signal.is_none()
    }
}

/// 强杀/优雅停止句柄（闭包封装，平台差异内聚）
#[derive(Clone)]
pub struct Killer {
    graceful: Arc<dyn Fn() + Send + Sync>,
    force: Arc<dyn Fn() + Send + Sync>,
}

impl Killer {
    pub fn new(
        graceful: impl Fn() + Send + Sync + 'static,
        force: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self { graceful: Arc::new(graceful), force: Arc::new(force) }
    }

    pub fn graceful(&self) {
        (self.graceful)()
    }

    pub fn force(&self) {
        (self.force)()
    }
}

/// 一次成功 spawn 的运行上下文
pub struct RunCtx {
    pub pid: u32,
    pub killer: Killer,
    /// 阻塞等待线程在进程退出后发送（oneshot）
    pub exit: oneshot::Receiver<ExitInfo>,
    /// PTY master 写端（FR-6.4 预留 stdin 通道；MVP 只读，不暴露 UI）
    pub pty_writer: Option<Box<dyn Write + Send>>,
}

pub(crate) fn default_cwd(cfg: &ServiceConfig) -> PathBuf {
    cfg.cwd.clone().unwrap_or_else(|| {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
    })
}

/// spawn 总入口：按 spawn_mode 分派；读线程在此启动并把行送入 hub。
pub(crate) fn spawn_service(
    cfg: &ServiceConfig,
    env: &std::collections::BTreeMap<String, String>,
    term: Option<&Arc<super::TermHandle>>,
    hub_tx: &tokio::sync::mpsc::UnboundedSender<HubMsg>,
) -> Result<RunCtx, String> {
    match cfg.spawn_mode {
        crate::config::SpawnMode::Pipe => spawn_pipe(cfg, env, hub_tx),
        crate::config::SpawnMode::Pty => spawn_pty(cfg, env, term, hub_tx),
    }
}

// ---------------------------------------------------------------------------
// 管道读线程：字节流 → 行 → hub
// ---------------------------------------------------------------------------

fn spawn_reader_thread(
    mut read: Box<dyn Read + Send>,
    stream: crate::events::Stream,
    tag_as_out: bool,
    hub_tx: tokio::sync::mpsc::UnboundedSender<HubMsg>,
) {
    std::thread::spawn(move || {
        let mut decoder = crate::log::LineDecoder::new();
        let mut buf = [0u8; 8192];
        loop {
            match read.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let effective = if tag_as_out { crate::events::Stream::Out } else { stream };
                    for line in decoder.feed(&buf[..n]) {
                        if hub_tx.send(HubMsg::Line(effective, line)).is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    log::warn!("日志读取失败：{e}");
                    break;
                }
            }
        }
        if let Some(line) = decoder.finish() {
            let _ = hub_tx.send(HubMsg::Line(
                if tag_as_out { crate::events::Stream::Out } else { stream },
                line,
            ));
        }
    });
}

fn spawn_pty_reader_thread(
    mut read: Box<dyn Read + Send>,
    term: Arc<super::TermHandle>,
    hub_tx: tokio::sync::mpsc::UnboundedSender<HubMsg>,
) {
    std::thread::spawn(move || {
        let mut decoder = crate::log::LineDecoder::new();
        let mut buf = [0u8; 8192];
        loop {
            match read.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    term.feed(&buf[..n]);
                    for line in decoder.feed(&buf[..n]) {
                        if hub_tx.send(HubMsg::Line(crate::events::Stream::Out, line)).is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    log::warn!("PTY 读取失败：{e}");
                    break;
                }
            }
        }
        if let Some(line) = decoder.finish() {
            let _ = hub_tx.send(HubMsg::Line(crate::events::Stream::Out, line));
        }
    });
}

// ---------------------------------------------------------------------------
// Unix 实现
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn unix_signal_killer(pid: u32, sig: libc::c_int) -> Killer {
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

#[cfg(unix)]
fn spawn_pipe(
    cfg: &ServiceConfig,
    env: &std::collections::BTreeMap<String, String>,
    hub_tx: &tokio::sync::mpsc::UnboundedSender<HubMsg>,
) -> Result<RunCtx, String> {
    use std::os::unix::process::CommandExt;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};

    let mut cmd = Command::new(&cfg.command);
    cmd.args(&cfg.args);
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
    Ok(RunCtx { pid, killer: unix_signal_killer(pid, libc::SIGTERM), exit: exit_rx, pty_writer: None })
}

#[cfg(unix)]
fn spawn_pty(
    cfg: &ServiceConfig,
    env: &std::collections::BTreeMap<String, String>,
    term: Option<&Arc<super::TermHandle>>,
    hub_tx: &tokio::sync::mpsc::UnboundedSender<HubMsg>,
) -> Result<RunCtx, String> {
    let term = term.cloned().ok_or("PTY 模式缺少终端句柄")?;
    let pair = native_pty_system()
        .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("打开 PTY 失败：{e}"))?;
    let mut cmd = CommandBuilder::new(&cfg.command);
    cmd.args(cfg.args.iter());
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

// ---------------------------------------------------------------------------
// Windows 实现
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod win {
    use super::*;
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        CloseHandle, DUPLICATE_SAME_ACCESS, HANDLE, HANDLE_FLAG_INHERIT,
    };
    use windows_sys::Win32::Security::{
        DuplicateHandle, SECURITY_ATTRIBUTES,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, GENERIC_READ, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        GenerateConsoleCtrlEvent, CTRL_BREAK_EVENT,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, GetExitCodeProcess, GetCurrentProcess, ResumeThread, TerminateProcess,
        WaitForSingleObject, CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED,
        CREATE_UNICODE_ENVIRONMENT, INFINITE, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
        STARTUPINFOW,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;

    pub(crate) struct JobWrap(pub HANDLE);
    impl Drop for JobWrap {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    pub(crate) fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Windows 命令行参数转义（MSVCRT 规则）
    pub(crate) fn quote_arg(s: &str) -> String {
        if !s.is_empty() && !s.chars().any(|c| c == ' ' || c == '\t' || c == '"') {
            return s.to_string();
        }
        let mut out = String::from("\"");
        let mut bs = 0usize;
        for c in s.chars() {
            if c == '\\' {
                bs += 1;
            } else if c == '"' {
                out.push_str(&"\\".repeat(bs * 2 + 1));
                out.push('"');
                bs = 0;
            } else {
                out.push_str(&"\\".repeat(bs));
                out.push(c);
                bs = 0;
            }
        }
        out.push_str(&"\\".repeat(bs * 2));
        out.push('"');
        out
    }

    pub(crate) fn build_env_block(env: &BTreeMap<String, String>) -> Vec<u16> {
        let mut block = Vec::new();
        for (k, v) in env {
            let mut kv = to_wide(&format!("{k}={v}"));
            kv.pop(); // 去掉 to_wide 附加的终止符，块末统一加
            block.extend_from_slice(&kv);
            block.push(0);
        }
        block.push(0);
        block
    }

    /// 创建 Job Object（KILL_ON_JOB_CLOSE，spike 已验证）
    pub(crate) fn create_job() -> Result<Arc<JobWrap>, String> {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err("CreateJobObjectW 失败".into());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                CloseHandle(job);
                return Err("SetInformationJobObject 失败".into());
            }
            Ok(Arc::new(JobWrap(job)))
        }
    }

    pub(crate) fn duplicate(h: HANDLE) -> Result<HANDLE, String> {
        unsafe {
            let mut dup: HANDLE = std::ptr::null_mut();
            let ok = DuplicateHandle(
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                h,
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                &mut dup,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            );
            if ok == 0 {
                return Err("DuplicateHandle 失败".into());
            }
            Ok(dup)
        }
    }

    pub(crate) fn terminate_process(h: HANDLE) {
        unsafe {
            TerminateProcess(h, 1);
        }
    }
}

#[cfg(windows)]
fn spawn_pipe(
    cfg: &ServiceConfig,
    env: &std::collections::BTreeMap<String, String>,
    hub_tx: &tokio::sync::mpsc::UnboundedSender<HubMsg>,
) -> Result<RunCtx, String> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
    use windows_sys::Win32::Storage::FileSystem::GENERIC_READ;
    use windows_sys::Win32::System::Pipes::CreatePipe;

    let job = win::create_job()?;
    let (job_for_wait, job_for_kill) = (job.clone(), job.clone());

    let cwd = default_cwd(cfg);

    let (exit_tx, exit_rx) = oneshot::channel();

    let result: Result<RunCtx, String> = unsafe {
        let mut sa = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        // stdout / stderr 管道
        let (mut out_rd, mut out_wr) = (std::ptr::null_mut(), std::ptr::null_mut());
        let (mut err_rd, mut err_wr) = (std::ptr::null_mut(), std::ptr::null_mut());
        if CreatePipe(&mut out_rd, &mut out_wr, &sa, 0) == 0 {
            return Err("CreatePipe(stdout) 失败".into());
        }
        if CreatePipe(&mut err_rd, &mut err_wr, &sa, 0) == 0 {
            return Err("CreatePipe(stderr) 失败".into());
        }
        // 我方读端不可继承
        SetHandleInformation(out_rd, HANDLE_FLAG_INHERIT, 0);
        SetHandleInformation(err_rd, HANDLE_FLAG_INHERIT, 0);
        // stdin → NUL
        let nul: Vec<u16> = win::to_wide("NUL");
        let stdin = CreateFileW(
            nul.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &mut sa,
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );

        let mut cmdline: Vec<u16> = win::to_wide(&format!(
            "{} {}",
            win::quote_arg(&cfg.command),
            cfg.args.iter().map(|a| win::quote_arg(a)).collect::<Vec<_>>().join(" ")
        ));
        let env_block = win::build_env_block(env);
        let cwd_w = win::to_wide(&cwd.to_string_lossy());

        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESTDHANDLES;
        si.hStdInput = stdin;
        si.hStdOutput = out_wr;
        si.hStdError = err_wr;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();

        let ok = CreateProcessW(
            std::ptr::null(),
            cmdline.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // bInheritHandles
            CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT,
            env_block.as_ptr() as *const core::ffi::c_void,
            if cfg.cwd.is_some() { cwd_w.as_ptr() } else { std::ptr::null() },
            &si,
            &mut pi,
        );
        if ok == 0 {
            return Err(format!(
                "无法启动 {}：os error {}",
                cfg.command,
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
            ));
        }
        let pid = pi.dwProcessId;

        // 先绑定 Job（挂起状态，无逃逸窗口）再恢复执行（spike 时序）
        if AssignProcessToJobObject(job.0, pi.hProcess) == 0 {
            TerminateProcess(pi.hProcess, 1);
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
            return Err("AssignProcessToJobObject 失败".into());
        }
        ResumeThread(pi.hThread);
        CloseHandle(pi.hThread);

        // 写端移交子进程：包装为 StdReader 后由读线程接管
        let out_reader = Box::new(std::process::ChildStdout::from_raw_handle(out_rd))
            as Box<dyn Read + Send>;
        let err_reader = Box::new(std::process::ChildStderr::from_raw_handle(err_rd))
            as Box<dyn Read + Send>;
        super::spawn_reader_thread(out_reader, crate::events::Stream::Out, false, hub_tx.clone());
        super::spawn_reader_thread(
            err_reader,
            crate::events::Stream::Err,
            cfg.redirect_stderr,
            hub_tx.clone(),
        );
        let _ = CloseHandle(out_wr);
        let _ = CloseHandle(err_wr);
        if stdin != std::ptr::null_mut() {
            let _ = CloseHandle(stdin);
        }

        // waiter：等待退出并取退出码；结束后关 hProcess 与 job（KILL_ON_JOB_CLOSE 兜底）
        let h_process = pi.hProcess;
        std::thread::spawn(move || {
            unsafe {
                WaitForSingleObject(h_process, INFINITE);
                let mut code: u32 = 0;
                GetExitCodeProcess(h_process, &mut code);
                CloseHandle(h_process);
                drop(job_for_wait);
            }
            let _ = exit_tx.send(ExitInfo { code: Some(code as i32), signal: None });
        });

        // 优雅停止：CTRL_BREAK 尽力尝试；失败回退终止直接子进程（强杀树交给 Job）
        let dup = win::duplicate(h_process)?;
        let job_kill = job_for_kill;
        let killer = Killer::new(
            move || unsafe {
                if GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) == 0 {
                    win::terminate_process(dup);
                }
            },
            move || unsafe {
                TerminateJobObject(job_kill.0, 1);
            },
        );
        Ok(RunCtx { pid, killer, exit: exit_rx, pty_writer: None })
    };
    result
}

#[cfg(windows)]
fn spawn_pty(
    cfg: &ServiceConfig,
    env: &std::collections::BTreeMap<String, String>,
    term: Option<&Arc<super::TermHandle>>,
    hub_tx: &tokio::sync::mpsc::UnboundedSender<HubMsg>,
) -> Result<RunCtx, String> {
    let term = term.cloned().ok_or("PTY 模式缺少终端句柄")?;
    let pair = native_pty_system()
        .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("打开 ConPTY 失败：{e}"))?;
    let mut cmd = CommandBuilder::new(&cfg.command);
    cmd.args(cfg.args.iter());
    cmd.cwd(default_cwd(cfg));
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("无法启动 {}：{e}", cfg.command))?;
    let pid = child.process_id().ok_or("PTY 子进程无 PID")?;
    drop(pair.slave);
    let reader = pair.master.try_clone_reader().map_err(|e| format!("PTY reader 失败：{e}"))?;
    let writer = pair.master.take_writer().map_err(|e| format!("PTY writer 失败：{e}"))?;
    super::spawn_pty_reader_thread(Box::new(reader), term, hub_tx.clone());

    // ConPTY 下无可用优雅停止：kill（TerminateProcess）即时终止，树由 ConPTY 会话回收
    let mut soft = child.clone_killer();
    let killer = Killer::new(
        move || {
            let _ = soft.kill();
        },
        move || {
            unsafe {
                let h = windows_sys::Win32::System::Threading::OpenProcess(
                    windows_sys::Win32::System::Threading::PROCESS_TERMINATE,
                    0,
                    pid,
                );
                if !h.is_null() {
                    TerminateProcess(h, 1);
                    CloseHandle(h);
                }
            }
        },
    );
    let (exit_tx, exit_rx) = oneshot::channel();
    std::thread::spawn(move || {
        let info = match child.wait() {
            Ok(st) => ExitInfo { code: Some(st.exit_code() as i32), signal: None },
            Err(_) => ExitInfo { code: Some(-1), signal: None },
        };
        let _ = exit_tx.send(info);
    });
    Ok(RunCtx { pid, killer, exit: exit_rx, pty_writer: Some(writer) })
}
