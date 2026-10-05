//! Win32 进程/Job Object 原生 API 薄封装（windows.rs 使用）。
//! Job Object 时序依据 spike `windows/job-object-kill-tree`。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::Arc;

use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, HANDLE,
};
use windows_sys::Win32::Security::DuplicateHandle;
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::TerminateProcess;

pub(super) struct JobWrap(pub HANDLE);
impl Drop for JobWrap {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub(super) fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// Windows 命令行参数转义（MSVCRT 规则）
pub(super) fn quote_arg(s: &str) -> String {
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

pub(super) fn build_env_block(env: &std::collections::BTreeMap<String, String>) -> Vec<u16> {
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
pub(super) fn create_job() -> Result<Arc<JobWrap>, String> {
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

pub(super) fn duplicate(h: HANDLE) -> Result<HANDLE, String> {
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

pub(super) fn terminate_process(h: HANDLE) {
    unsafe {
        TerminateProcess(h, 1);
    }
}
