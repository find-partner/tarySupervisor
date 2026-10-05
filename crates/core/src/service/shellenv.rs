//! 子进程环境兜底（GUI 宿主 PATH 精简问题）。
//!
//! macOS/Linux 从 Finder/启动台（launchd）拉起的 GUI 应用只继承
//! `/usr/bin:/bin:/usr/sbin:/sbin`，用户 shell 配置（.zshrc/.bashrc）追加的
//! Homebrew / nvm 等路径全部缺失，`#!/usr/bin/env node` 类解释器查找直接
//! `env: node: No such file or directory`（exit 127）。
//!
//! 两个职责（CONFIG-SPEC §3.5）：
//! 1. PATH 兜底注入：服务未显式配置 `environment=PATH=...` 时，一次性通过
//!    用户登录 shell（交互+登录，等价终端）取回 PATH 供子进程使用；
//!    shell 配置显式设置 PATH 时以配置为准。不改变 FR-3.6（目标命令仍不经 shell 包装）。
//! 2. shell 模式包装：`shell=true` 时目标命令经 `$SHELL -ilc` 启动，
//!    argv 经 POSIX 单引号重组，解析结果与直接 exec 一致。

use std::collections::BTreeMap;

/// 服务未显式配置 PATH 时注入兜底 PATH；幂等（已含 PATH 即跳过）。
///
/// Windows 桌面进程本就从注册表获得完整 PATH，无需处理。
pub fn inject_fallback_path(env: &mut BTreeMap<String, String>) {
    if env.contains_key("PATH") {
        return;
    }
    #[cfg(unix)]
    if let Some(p) = login_shell_path() {
        env.insert("PATH".into(), p);
        return;
    }
    #[cfg(unix)]
    inject_homebrew_fallback(env);
}

/// 预热 PATH 缓存：ServiceManager 创建时后台调用，避免首个服务启动时同步等待。
pub fn warm() {
    #[cfg(unix)]
    std::thread::Builder::new()
        .name("traysv-shellpath".into())
        .spawn(login_shell_path)
        .ok();
}

/// 登录 shell 的 PATH（进程内缓存，改动 shell 配置需重启应用生效）。
#[cfg(unix)]
pub fn login_shell_path() -> Option<String> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE.get_or_init(query_login_shell_path).clone()
}

/// 探测一次登录 shell 的 PATH：`$SHELL -ilc 'printf %s "$PATH"'`。
/// 交互+登录的组合使 .zshrc/.bashrc（nvm 等常驻初始化所在地）参与 PATH 构建；
/// rc 输出的杂讯按「末行」剔除。超时/失败返回 None（调用方回退静态目录拼接）。
#[cfg(unix)]
fn query_login_shell_path() -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::{Duration, Instant};

    const TIMEOUT: Duration = Duration::from_secs(5);

    let mut child = Command::new(shell_binary())
        .args(["-ilc", "printf %s \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = Read::read_to_string(&mut out, &mut buf);
        let _ = tx.send(buf);
    });
    let deadline = Instant::now() + TIMEOUT;
    let captured = {
        let remain = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remain) {
            Ok(s) => s,
            Err(RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                log::warn!("登录 shell PATH 探测超时（{TIMEOUT:?}），回退静态 PATH 兜底");
                return None;
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = child.wait();
                return None;
            }
        }
    };
    let _ = child.wait();
    // rc 文件可能向 stdout 打印杂讯；PATH 是最后一条命令的输出 → 取末行
    captured
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// 登录 shell 不可用时的静态兜底：Homebrew / 本地惯例目录置于继承 PATH 之前。
#[cfg(unix)]
fn inject_homebrew_fallback(env: &mut BTreeMap<String, String>) {
    use std::path::Path;

    const FALLBACK_DIRS: &[&str] = &[
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/usr/local/sbin",
    ];
    let inherited = std::env::var("PATH").unwrap_or_default();
    let extra: Vec<&str> = FALLBACK_DIRS
        .iter()
        .copied()
        .filter(|d| Path::new(d).is_dir() && !inherited.split(':').any(|p| p == *d))
        .collect();
    let path = if extra.is_empty() {
        inherited
    } else {
        format!("{}:{inherited}", extra.join(":"))
    };
    if !path.is_empty() {
        env.insert("PATH".into(), path);
    }
}

/// 用户的 shell：`$SHELL` 优先，缺失时回退 passwd 数据库（与 portable-pty 一致）。
#[cfg(unix)]
pub fn shell_binary() -> String {
    if let Ok(s) = std::env::var("SHELL") {
        if !s.is_empty() {
            return s;
        }
    }
    passwd_shell().unwrap_or_else(|| "/bin/sh".into())
}

#[cfg(unix)]
fn passwd_shell() -> Option<String> {
    use std::ffi::CStr;
    unsafe {
        let ent = libc::getpwuid(libc::getuid());
        if ent.is_null() {
            return None;
        }
        let s = CStr::from_ptr((*ent).pw_shell).to_str().ok()?;
        (!s.is_empty()).then(|| s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_path_is_respected() {
        let mut env = BTreeMap::from([("PATH".into(), "/custom".into())]);
        inject_fallback_path(&mut env);
        assert_eq!(env["PATH"], "/custom", "服务显式配置的 PATH 不得被覆盖");
    }

    #[cfg(unix)]
    #[test]
    fn missing_path_gets_non_empty_value() {
        let mut env = BTreeMap::new();
        inject_fallback_path(&mut env);
        let p = env.get("PATH").expect("应注入 PATH");
        assert!(!p.is_empty());
        assert!(p.split(':').all(|d| !d.is_empty()), "PATH 段非空：{p}");
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_path_resolves() {
        // 本机/CI（macOS、Linux 容器）均具备可用的 $SHELL 或 passwd shell
        let p = login_shell_path().expect("应能从登录 shell 取回 PATH");
        assert!(p.contains('/'), "PATH 形如目录列表：{p}");
    }
}
