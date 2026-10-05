//! Windows spawn 实现：挂起创建 → 绑定 Job Object → 恢复执行（spike 时序）；
//! PTY 走 ConPTY。

use std::io::Read;
use std::os::windows::io::FromRawHandle;
use std::sync::Arc;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tokio::sync::oneshot;
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, SetHandleInformation, HANDLE_FLAG_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{GenerateConsoleCtrlEvent, CTRL_BREAK_EVENT};
use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, TerminateJobObject};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, ResumeThread, TerminateProcess, WaitForSingleObject,
    CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, INFINITE,
    PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW,
};

use super::winapi;
use super::{default_cwd, resolve_command, spawn_pty_reader_thread, spawn_reader_thread, ExitInfo, Killer, RunCtx};
use super::HubMsg;
use crate::config::ServiceConfig;

pub(super) fn spawn_pipe(
    cfg: &ServiceConfig,
    env: &std::collections::BTreeMap<String, String>,
    hub_tx: &tokio::sync::mpsc::UnboundedSender<HubMsg>,
) -> Result<RunCtx, String> {
    let job = winapi::create_job()?;
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
        let nul: Vec<u16> = winapi::to_wide("NUL");
        let stdin = CreateFileW(
            nul.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &mut sa,
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );

        let (prog, args) = resolve_command(cfg);
        let mut cmdline: Vec<u16> = winapi::to_wide(&format!(
            "{} {}",
            winapi::quote_arg(&prog),
            args.iter().map(|a| winapi::quote_arg(a)).collect::<Vec<_>>().join(" ")
        ));
        // CREATE_UNICODE_ENVIRONMENT 是整体替换：环境块 = 继承本进程环境 + 服务条目覆盖，
        // 与 Unix Command::env 的「继承+覆盖」语义一致。仅传服务条目会得到空环境，
        // 子进程缺 SystemRoot/PATH 秒退（PATH 兜底注入在 Windows 明确不做，见 shellenv）。
        let mut full_env: std::collections::BTreeMap<String, String> = std::env::vars_os()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
            .collect();
        full_env.extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
        let env_block = winapi::build_env_block(&full_env);
        let cwd_w = winapi::to_wide(&cwd.to_string_lossy());

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

        // 写端移交子进程：管道读端包装为 File 后由读线程接管
        let out_reader = Box::new(std::fs::File::from_raw_handle(out_rd))
            as Box<dyn Read + Send>;
        let err_reader = Box::new(std::fs::File::from_raw_handle(err_rd))
            as Box<dyn Read + Send>;
        spawn_reader_thread(out_reader, crate::events::Stream::Out, false, hub_tx.clone());
        spawn_reader_thread(
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
        let h_process = winapi::SendHandle(pi.hProcess);
        std::thread::spawn(move || {
            let code = {
                WaitForSingleObject(h_process.raw(), INFINITE);
                let mut code: u32 = 0;
                GetExitCodeProcess(h_process.raw(), &mut code);
                CloseHandle(h_process.raw());
                code
            };
            drop(job_for_wait);
            let _ = exit_tx.send(ExitInfo { code: Some(code as i32), signal: None });
        });

        // 优雅停止：CTRL_BREAK 尽力尝试；失败回退终止直接子进程（强杀树交给 Job）
        let dup = winapi::SendHandle(winapi::duplicate(h_process.0)?);
        let job_kill = job_for_kill;
        let killer = Killer::new(
            move || {
                if GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) == 0 {
                    winapi::terminate_process(dup.raw());
                }
            },
            move || {
                TerminateJobObject(job_kill.0, 1);
            },
        );
        Ok(RunCtx { pid, killer, exit: exit_rx, pty_writer: None })
    };
    result
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
        .map_err(|e| format!("打开 ConPTY 失败：{e}"))?;
    let (prog, args) = resolve_command(cfg);
    let mut cmd = CommandBuilder::new(&prog);
    cmd.args(args.iter());
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
    spawn_pty_reader_thread(Box::new(reader), term, hub_tx.clone());

    // ConPTY 下无可用优雅停止：kill（TerminateProcess）即时终止，树由 ConPTY 会话回收
    // portable-pty 的 killer 需要 &mut 调用，用 Mutex 适配 Killer 的 Fn 约束
    let soft = std::sync::Mutex::new(child.clone_killer());
    let killer = Killer::new(
        move || {
            if let Ok(mut k) = soft.lock() {
                let _ = k.kill();
            }
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
