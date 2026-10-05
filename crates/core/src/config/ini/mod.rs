//! INI 编解码（CONFIG-SPEC）：手写解析器 → 语义模型；手写序列化保证转义可控。
//! 解析见 parse.rs / program.rs，测试见 tests.rs。
//!
//! 解析语义（刻意最小化，不借助 rust-ini 以保证值原样保留）：
//! - 值不做任何转义/去引号处理，`\` 与 `"` 原样保留（CONFIG-SPEC §3.4 第 3 条的必然要求）；
//! - `#` / `;` 行注释；节与键按文件顺序保留；
//! - 节：`[traysupervisor]`（可选）与 `[program:<id>]`；未知节/未知键 = 校验错误（§1/§7.1）；
//! - matcher 以 `matcher.N.pattern/template/max_items` 编号键表达，N 从 1 连续递增（§5）；
//! - 保存：先写临时文件再原子改名；Unix 权限 0600（NFR-5）。

mod parse;
mod program;
#[cfg(test)]
mod tests;

use std::path::Path;

use super::{format_byte_size, join_command_line, serialize_environment, AppConfig, SpawnMode};

pub use parse::parse_ini_str;

pub fn load_app_config(path: &Path) -> Result<AppConfig, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取配置失败：{e}"))?;
    parse_ini_str(&text, path.parent())
}

/// 序列化为 INI 文本（CONFIG-SPEC §6 示例风格）。
pub fn to_ini_string(cfg: &AppConfig) -> String {
    let mut out = String::new();
    out.push_str("# TraySupervisor 配置（supervisord 风格）；完整规范见 docs/CONFIG-SPEC.md\n");
    out.push_str("\n[traysupervisor]\n");
    out.push_str(&format!("log_lines={}\n", cfg.settings.log_lines));
    out.push_str(&format!("notify_on_failure={}\n", cfg.settings.notify_on_failure));
    for svc in &cfg.services {
        out.push_str(&format!("\n[program:{}]\n", svc.id));
        if !svc.name.is_empty() && svc.name != svc.id {
            out.push_str(&format!("name={}\n", svc.name));
        }
        out.push_str(&format!("command={}\n", join_command_line(&svc.command, &svc.args)));
        if let Some(dir) = &svc.cwd {
            out.push_str(&format!("directory={}\n", dir.display()));
        }
        out.push_str(&format!("autostart={}\n", svc.autostart));
        out.push_str(&format!("autorestart={}\n", svc.autorestart.as_str()));
        out.push_str(&format!("startsecs={}\n", svc.startsecs));
        out.push_str(&format!("startretries={}\n", svc.startretries));
        out.push_str(&format!("stopwaitsecs={}\n", svc.stop_timeout_secs));
        if svc.spawn_mode != SpawnMode::Pipe {
            out.push_str(&format!("spawn_mode={}\n", svc.spawn_mode.as_str()));
        }
        if svc.shell {
            out.push_str("shell=true\n");
        }
        if svc.redirect_stderr {
            out.push_str("redirect_stderr=true\n");
        }
        if svc.term_interactive {
            out.push_str("term_interactive=true\n");
        }
        if !svc.env.is_empty() {
            out.push_str(&format!("environment={}\n", serialize_environment(&svc.env)));
        }
        if let Some(f) = &svc.log.file {
            out.push_str(&format!("stdout_logfile={}\n", f.path.display()));
            out.push_str(&format!("stdout_logfile_maxbytes={}\n", format_byte_size(f.max_bytes)));
            out.push_str(&format!("stdout_logfile_backups={}\n", f.max_files));
            if f.compress {
                out.push_str("logfile_compress=true\n");
            }
        }
        for (i, m) in svc.matchers.iter().enumerate() {
            out.push_str(&format!("matcher.{}.pattern={}\n", i + 1, m.pattern));
            out.push_str(&format!("matcher.{}.template={}\n", i + 1, m.template));
            out.push_str(&format!("matcher.{}.max_items={}\n", i + 1, m.max_items));
        }
    }
    out
}

/// 原子保存：临时文件 + 改名；Unix 权限 0600（NFR-5）。
pub fn save_app_config(cfg: &AppConfig, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let text = to_ini_string(cfg);
    let tmp = path.with_extension("ini.tmp");
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| format!("写入配置失败：{e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("保存配置失败：{e}"))?;
    Ok(())
}

/// 生成默认（空服务）配置并写盘；返回内容。
pub fn ensure_default_config(path: &Path) -> Result<AppConfig, String> {
    if path.exists() {
        return load_app_config(path);
    }
    let cfg = AppConfig::default();
    save_app_config(&cfg, path)?;
    Ok(cfg)
}
