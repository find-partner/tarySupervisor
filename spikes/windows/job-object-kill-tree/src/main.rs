// Spike 2：验证 Windows Job Object 进程树终止（SRS FR-3.3 / HLD D-5）
//
// 仅在 Windows 上运行：cargo run --release
//
// 场景：
//   1. 创建 Job Object（KILL_ON_JOB_CLOSE）
//   2. 以 CREATE_SUSPENDED 创建 cmd.exe，命令行内部形成孙进程链：
//      外层 cmd → 内层 cmd → ping（ping -t 持续运行）
//   3. AssignProcessToJobObject 绑定 + ResumeThread 恢复执行
//   4. TerminateJobObject 整树终止
//   5. 用 tasklist 验证 ping.exe 已全部退出 => PASS
//
// 预期输出末尾为 "[RESULT] PASS"，证明 Job Object 方案可整树杀掉孙子进程，
// 这是本产品在 Windows 上"停止服务"的核心机制。

#[cfg(not(windows))]
fn main() {
    println!("本 spike 仅支持 Windows，请在 Windows 机器上运行：cargo run --release");
}

#[cfg(windows)]
fn main() {
    std::process::exit(win::run());
}

#[cfg(windows)]
mod win {
    use std::ffi::OsStr;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::process::Command;
    use std::thread::sleep;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, ResumeThread, CREATE_SUSPENDED, PROCESS_INFORMATION, STARTUPINFOW,
    };

    fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// 用 tasklist 统计 ping.exe 进程数
    fn ping_count() -> usize {
        let out = Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq ping.exe", "/FO", "CSV", "/NH"])
            .output()
            .expect("无法执行 tasklist");
        let s = String::from_utf8_lossy(&out.stdout);
        s.lines()
            .filter(|l| l.to_lowercase().contains("ping.exe"))
            .count()
    }

    pub fn run() -> i32 {
        unsafe {
            // 1. 创建 Job Object 并设置 KILL_ON_JOB_CLOSE
            let job: HANDLE = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                eprintln!("[ERROR] CreateJobObjectW 失败");
                return 1;
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                eprintln!("[ERROR] SetInformationJobObject 失败");
                return 1;
            }
            println!("[1] Job Object 已创建（KILL_ON_JOB_CLOSE）");

            // 2. 以挂起方式创建进程树：外层 cmd → 内层 cmd → ping -t
            let mut cmdline = to_wide(
                "cmd.exe /c \"cmd /c ping -t 127.0.0.1 & timeout /t 600 /nobreak\"",
            );
            let mut si: STARTUPINFOW = zeroed();
            si.cb = size_of::<STARTUPINFOW>() as u32;
            let mut pi: PROCESS_INFORMATION = zeroed();

            let ok = CreateProcessW(
                std::ptr::null(),
                cmdline.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_SUSPENDED,
                std::ptr::null(),
                std::ptr::null(),
                &si,
                &mut pi,
            );
            if ok == 0 {
                eprintln!("[ERROR] CreateProcessW 失败");
                return 1;
            }
            println!("[2] 已挂起创建 cmd.exe（PID={}），进程链: cmd → cmd → ping", pi.dwProcessId);

            // 3. 绑定 Job Object 后再恢复执行（先挂起再绑定，保证无逃逸窗口）
            if AssignProcessToJobObject(job, pi.hProcess) == 0 {
                eprintln!("[ERROR] AssignProcessToJobObject 失败");
                return 1;
            }
            ResumeThread(pi.hThread);
            println!("[3] 已绑定 Job Object 并恢复执行，等待 3s 让孙进程 spawn...");
            sleep(Duration::from_secs(3));

            let before = ping_count();
            println!("[4] 终止前 ping.exe 数量 = {before}（预期 ≥ 1）");

            // 4. 整树终止
            TerminateJobObject(job, 1);
            sleep(Duration::from_secs(2));

            // 5. 验证
            let after = ping_count();
            println!("[5] 终止后 ping.exe 数量 = {after}（预期 0）");

            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
            let _ = CloseHandle(job); // KILL_ON_JOB_CLOSE 兜底

            if before >= 1 && after == 0 {
                println!("[RESULT] PASS —— Job Object 成功终止整棵进程树（含孙进程）");
                0
            } else {
                println!("[RESULT] FAIL —— 请检查终端输出；注意不要用管理员终端运行普通进程冲突等场景");
                2
            }
        }
    }
}
