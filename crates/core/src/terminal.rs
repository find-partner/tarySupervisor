//! 系统终端启动器（FR-2.16 / HLD D-20）：在系统终端 App 中打开指定目录。
//! 平台映射——macOS `open -a Terminal <dir>`；Windows `cmd /c start "" /D <dir> cmd`；
//! Linux 按候选列表尝试（gnome-terminal → konsole → xfce4-terminal →
//! x-terminal-emulator → xterm，后两者以 cwd 继承目录）。
//! 命令构造为纯函数可单测（不实际 spawn，避免测试期弹出真窗口）。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 构造"在系统终端打开 dir"的候选命令列表（program + args，按优先级排序）。
/// macOS/Windows 恒为单项；Linux 逐候选尝试。纯函数，便于单测。
pub fn terminal_commands(dir: &Path) -> Vec<(PathBuf, Vec<String>)> {
    let dir_str = dir.to_string_lossy().to_string();
    #[cfg(target_os = "macos")]
    {
        vec![(
            PathBuf::from("open"),
            vec!["-a".into(), "Terminal".into(), dir_str],
        )]
    }
    #[cfg(windows)]
    {
        // start 的第一个带引号参数是窗口标题，用空串占位；/D 设定新窗口起始目录
        vec![(
            PathBuf::from("cmd"),
            vec![
                "/c".into(),
                "start".into(),
                String::new(),
                "/D".into(),
                dir_str,
                "cmd".into(),
            ],
        )]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // 各终端的目录参数不同，按桌面环境常见度排序；兜底项无目录参数，靠 cwd 继承
        vec![
            (
                PathBuf::from("gnome-terminal"),
                vec![format!("--working-directory={dir_str}")],
            ),
            (
                PathBuf::from("konsole"),
                vec!["--workdir".into(), dir_str.clone()],
            ),
            (
                PathBuf::from("xfce4-terminal"),
                vec![format!("--working-directory={dir_str}")],
            ),
            (PathBuf::from("x-terminal-emulator"), vec![]),
            (PathBuf::from("xterm"), vec![]),
        ]
    }
}

/// 在系统终端 App 中打开 `dir`（不存在则先创建）。任一候选启动成功即返回，
/// 全部失败（如 Linux 无已装终端）时报最后一个错误。
pub fn open_in_terminal(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败：{e}"))?;
    let mut last_err = String::new();
    for (program, args) in terminal_commands(dir) {
        // cwd 设为目标目录：GNOME/Konsole/Xfce 有目录参数，兜底终端靠继承
        match Command::new(&program).args(&args).current_dir(dir).spawn() {
            Ok(_) => return Ok(()),
            Err(e) => last_err = format!("{}：{e}", program.to_string_lossy()),
        }
    }
    Err(format!("打开终端失败（{last_err}），本机可能未安装受支持的终端 App"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 构造_目录参数正确() {
        let dir = std::env::temp_dir().join("traysv_term_test");
        let cmds = terminal_commands(&dir);
        assert!(!cmds.is_empty(), "候选列表不应为空");
        #[cfg(target_os = "macos")]
        {
            let (program, args) = &cmds[0];
            assert_eq!(program.to_string_lossy(), "open");
            assert_eq!(args, &["-a", "Terminal", dir.to_string_lossy().as_ref()]);
        }
        #[cfg(windows)]
        {
            let (program, args) = &cmds[0];
            assert_eq!(program.to_string_lossy(), "cmd");
            assert!(args.windows(2).any(|w| w[0] == "/D" && w[1] == dir.to_string_lossy()));
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            assert!(cmds.len() >= 2, "Linux 应有多个候选终端");
        }
    }
}
