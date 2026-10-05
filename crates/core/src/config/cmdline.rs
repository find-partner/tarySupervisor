//! command 单行切分/反切分（CONFIG-SPEC §3.4，对 shlex 的 Windows 友好偏离）。
//!
//! 规则：
//! 1. 空白（空格/Tab）分隔 token；
//! 2. 双引号包裹的部分整体为一个 token，内部空白不切分；
//! 3. `\` 仅在其后紧跟 `"`、`\` 或空白时作为转义符，其余情况字面保留
//!    —— Windows 路径 `C:\Users\me\app.exe` 无需引号即可使用；
//! 4. 第一个 token 为程序，其余为参数数组（FR-3.6：不经 shell）。

/// 切分单行命令为（程序, 参数数组）。空行/无 token 返回 Err。
pub fn split_command_line(s: &str) -> Result<(String, Vec<String>), String> {
    let tokens = tokenize(s)?;
    match tokens.split_first() {
        Some((prog, args)) => Ok((prog.clone(), args.to_vec())),
        None => Err("command 为空".to_string()),
    }
}

/// 将（程序, 参数数组）拼回单行 command（序列化到 INI 时用）。
pub fn join_command_line(prog: &str, args: &[String]) -> String {
    let mut parts = vec![quote_token(prog)];
    for a in args {
        parts.push(quote_token(a));
    }
    parts.join(" ")
}

/// POSIX shell 单引号转义（`shell=true` 模式重组命令行用）。
/// 单引号内无任何转义语义，闭合再接 `'\\''` 即可表达字面单引号，
/// 保证 shell 解析出的 argv 与直接 exec 完全一致。
pub fn shell_quote_posix(tok: &str) -> String {
    format!("'{}'", tok.replace('\'', r"'\''"))
}

/// 粘贴整行解析（FR-2.12 / DESIGN §3）：按 §3.4 切分 token 后，
/// 从头扫描形如 `KEY=VALUE` 的前缀 token 记为环境变量（Unix 惯例），
/// 第一个不符合该形态的 token 即程序，其后全部为参数。
///
/// - 环境变量条目 `expand=false`（字面量直传）、无平台标签；
/// - 重复 KEY 保留最后值、位置取首次出现（与 §4.3 就地覆盖语义一致）；
/// - 整行全为 `KEY=VALUE`（没有程序）→ Err「缺少命令」；
/// - 无前缀环境变量 → 与 [`split_command_line`] 等价（返回空 env 表）。
///
/// 返回（环境变量条目, 剩余命令行单行）。
pub fn split_env_prefixed_line(
    line: &str,
) -> Result<(Vec<crate::config::EnvEntry>, String), String> {
    let tokens = tokenize(line)?;
    let mut prefix_end = 0;
    while prefix_end < tokens.len() {
        match tokens[prefix_end].split_once('=') {
            Some((k, _)) if is_env_key(k) => prefix_end += 1,
            _ => break,
        }
    }
    if prefix_end == tokens.len() {
        let sample = tokens.first().map(String::as_str).unwrap_or("");
        return Err(format!(
            "缺少命令——「{sample}=…」均为环境变量赋值，其后应跟程序与参数"
        ));
    }
    // 前缀 token → 有序 KV（重复 KEY 就地覆盖）
    let mut env: Vec<crate::config::EnvEntry> = Vec::new();
    for tok in &tokens[..prefix_end] {
        let (k, v) = tok
            .split_once('=')
            .expect("前缀 token 已通过 KEY= 形态校验");
        if let Some(slot) = env.iter_mut().find(|e| e.key == k) {
            slot.value = v.to_string();
        } else {
            env.push(crate::config::EnvEntry {
                key: k.to_string(),
                value: v.to_string(),
                expand: false,
                platform: None,
            });
        }
    }
    let (prog, args) = tokens[prefix_end..]
        .split_first()
        .expect("prefix_end < len");
    Ok((env, join_command_line(prog, args)))
}

/// 合法环境变量键名：`[A-Za-z_][A-Za-z0-9_]*`
fn is_env_key(k: &str) -> bool {
    let mut chars = k.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn tokenize(s: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut has_content = false;
    let mut in_quotes = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() && matches!(chars[i + 1], '"' | '\\' | ' ' | '\t') {
            // 规则 3：仅转义 `"`、`\`、空白
            cur.push(chars[i + 1]);
            has_content = true;
            i += 2;
        } else if c == '"' {
            in_quotes = !in_quotes;
            has_content = true;
            i += 1;
        } else if c.is_whitespace() && !in_quotes {
            if has_content {
                tokens.push(std::mem::take(&mut cur));
                has_content = false;
            }
            i += 1;
        } else {
            cur.push(c);
            has_content = true;
            i += 1;
        }
    }
    if in_quotes {
        return Err("引号未闭合".to_string());
    }
    if has_content {
        tokens.push(cur);
    }
    Ok(tokens)
}

/// 单 token 反序列化安全：含空白/引号/空串时加引号；引号内做最小转义。
fn quote_token(tok: &str) -> String {
    let needs_quote = tok.is_empty() || tok.chars().any(|c| c.is_whitespace() || c == '"');
    if !needs_quote {
        return tok.to_string();
    }
    let mut out = String::with_capacity(tok.len() + 2);
    out.push('"');
    let chars: Vec<char> = tok.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '"' => {
                out.push('\\');
                out.push('"');
            }
            '\\' => {
                // 与解析规则对称：当下一个字符是 `"` / `\` / 空白（或已是末尾）时，
                // 解析器会把 `\` 当转义符，因此必须双写；其余字面保留
                // （让 `C:\Users\me` 这类路径保持可读）
                let next_is_special = chars
                    .get(i + 1)
                    .is_none_or(|&n| n == '"' || n == '\\' || n.is_whitespace());
                if next_is_special {
                    out.push('\\');
                }
                out.push('\\');
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_windows_path_without_quotes() {
        let (prog, args) = split_command_line(r"C:\Users\me\app.exe --flag value").unwrap();
        assert_eq!(prog, r"C:\Users\me\app.exe");
        assert_eq!(args, vec!["--flag", "value"]);
    }

    #[test]
    fn splits_quoted_spaces() {
        let (prog, args) =
            split_command_line(r#"python -m http.server 8000 --directory "C:\work\docs""#).unwrap();
        assert_eq!(prog, "python");
        assert_eq!(
            args,
            vec!["-m", "http.server", "8000", "--directory", r"C:\work\docs"]
        );
    }

    #[test]
    fn empty_quoted_token() {
        let (prog, args) = split_command_line(r#"echo "" x"#).unwrap();
        assert_eq!(prog, "echo");
        assert_eq!(args, vec!["", "x"]);
    }

    #[test]
    fn escaped_quote_inside_quotes() {
        let (prog, args) = split_command_line(r#"printf "a \"b\" c""#).unwrap();
        assert_eq!(prog, "printf");
        assert_eq!(args, vec![r#"a "b" c"#]);
    }

    #[test]
    fn progress_bar_command_from_system_test_splits() {
        // docs/SYSTEM-TEST.md ST-15 的示例命令：引号内含 \r、\033、单引号，必须原样保真
        let line = r#"python3 -u -c "import time,sys;[(sys.stdout.write('\r\033[32m'+'█'*(i%40)+'\033[0m %d'%i),sys.stdout.flush(),time.sleep(0.1)) for i in range(80)]""#;
        let (prog, args) = split_command_line(line).unwrap();
        assert_eq!(prog, "python3");
        assert_eq!(args[0], "-u");
        assert_eq!(args[1], "-c");
        assert_eq!(
            args[2],
            r"import time,sys;[(sys.stdout.write('\r\033[32m'+'█'*(i%40)+'\033[0m %d'%i),sys.stdout.flush(),time.sleep(0.1)) for i in range(80)]"
        );
    }

    #[test]
    fn unterminated_quote_is_error() {
        assert!(split_command_line(r#"echo "abc"#).is_err());
    }

    #[test]
    fn empty_input_is_error() {
        assert!(split_command_line("   ").is_err());
        assert!(split_command_line("").is_err());
    }

    #[test]
    fn trailing_backslash_in_quoted_token() {
        // §3.4：`\` 在 `"` 前是转义符 → 手写需双写反斜杠表达“字面反斜杠+结束引号”
        let (prog, args) = split_command_line(r#""C:\dir end\\" x"#).unwrap();
        assert_eq!(prog, r"C:\dir end\");
        assert_eq!(args, vec!["x"]);
    }

    fn roundtrip(prog: &str, args: &[&str]) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let line = join_command_line(prog, &args);
        let (p2, a2) = split_command_line(&line).unwrap();
        assert_eq!(p2, prog, "line = {line:?}");
        assert_eq!(a2, args, "line = {line:?}");
    }

    #[test]
    fn shell_quote_posix_escapes_exactly() {
        // 空串、空格、单引号（'\'' 闭合转义）、双引号与反斜杠（单引号内字面保留）
        assert_eq!(shell_quote_posix(""), "''");
        assert_eq!(shell_quote_posix("a b"), "'a b'");
        assert_eq!(shell_quote_posix("it's"), r"'it'\''s'");
        assert_eq!(shell_quote_posix(r#"say "hi""#), r#"'say "hi"'"#);
        assert_eq!(shell_quote_posix(r"C:\path\"), r"'C:\path\'");
    }

    #[test]
    fn join_roundtrip_plain() {
        roundtrip("python", &["-m", "http.server", "8000"]);
    }

    #[test]
    fn join_roundtrip_windows_path() {
        roundtrip(r"C:\Users\me\app.exe", &["--dir", r"C:\work\docs"]);
    }

    #[test]
    fn join_roundtrip_spaces_and_quotes() {
        roundtrip(
            r"C:\Program Files\app.exe",
            &["a b", r#"q "inner" q"#, "", r"C:\end\"],
        );
        roundtrip("/bin/echo", &["hello world"]);
    }

    // ---------- split_env_prefixed_line（FR-2.12）----------

    fn envs(line: &str) -> (Vec<(String, String)>, String) {
        let (env, cmd) = split_env_prefixed_line(line).unwrap();
        (env.into_iter().map(|e| (e.key, e.value)).collect(), cmd)
    }

    #[test]
    fn paste_spec_example_splits_env_prefix() {
        let (env, cmd) = envs(
            "ENABLE_CORS=true USE_PROXY=true PROXY_URL=http://127.0.0.1:7890 open-websearch --port=1123",
        );
        assert_eq!(
            env,
            vec![
                ("ENABLE_CORS".into(), "true".into()),
                ("USE_PROXY".into(), "true".into()),
                ("PROXY_URL".into(), "http://127.0.0.1:7890".into()),
            ]
        );
        assert_eq!(cmd, "open-websearch --port=1123");
    }

    #[test]
    fn paste_quoted_value_keeps_spaces() {
        let (env, cmd) = envs(r#"FOO="hello world" python -m http.server 8000"#);
        assert_eq!(env, vec![("FOO".into(), "hello world".into())]);
        assert_eq!(cmd, "python -m http.server 8000");
    }

    #[test]
    fn paste_kv_after_program_is_arg() {
        let (env, cmd) = envs("python FOO=bar x");
        assert!(env.is_empty());
        assert_eq!(cmd, "python FOO=bar x");
    }

    #[test]
    fn paste_all_kv_is_error() {
        assert!(split_env_prefixed_line("FOO=bar").is_err());
        assert!(split_env_prefixed_line("FOO=bar A=1")
            .unwrap_err()
            .contains("缺少命令"));
    }

    #[test]
    fn paste_plain_command_unchanged() {
        let (env, cmd) = envs(r#"python -m http.server 8000 --directory "C:\work\docs""#);
        assert!(env.is_empty());
        // join 可能去掉不必要的引号（roundtrip 安全）：比较切分结果
        let (p, a) = split_command_line(&cmd).unwrap();
        assert_eq!(p, "python");
        assert_eq!(
            a,
            vec!["-m", "http.server", "8000", "--directory", r"C:\work\docs"]
        );
    }

    #[test]
    fn paste_duplicate_key_last_wins_first_position() {
        let (env, cmd) = envs("A=1 B=2 A=3 run");
        assert_eq!(
            env,
            vec![("A".into(), "3".into()), ("B".into(), "2".into())]
        );
        assert_eq!(cmd, "run");
    }

    #[test]
    fn paste_empty_value_and_flags() {
        // KEY=（空值）合法；--flag=value 不是环境变量（键含 '-'）
        let (env, cmd) = envs("EMPTY= app --flag=value");
        assert_eq!(env, vec![("EMPTY".into(), "".into())]);
        assert_eq!(cmd, "app --flag=value");
    }

    #[test]
    fn paste_env_key_must_be_identifier() {
        // 数字开头 / 含连字符的「KEY=」不算前缀环境变量，整体视为程序
        assert!(split_env_prefixed_line("1A=b run").unwrap().0.is_empty());
        assert!(split_env_prefixed_line("A-B=c run").unwrap().0.is_empty());
    }

    #[test]
    fn paste_roundtrip_into_split() {
        let line = "K=\"v v\" node --max-old-space-size=512 main.js";
        let (env, cmd) = split_env_prefixed_line(line).unwrap();
        let (prog, args) = split_command_line(&cmd).unwrap();
        assert_eq!(prog, "node");
        assert_eq!(args, vec!["--max-old-space-size=512", "main.js"]);
        assert_eq!(env[0].key, "K");
        assert_eq!(env[0].value, "v v");
        assert!(!env[0].expand);
    }
}
