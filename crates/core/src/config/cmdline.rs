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
        assert_eq!(args, vec!["-m", "http.server", "8000", "--directory", r"C:\work\docs"]);
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
        assert_eq!(args[2], r"import time,sys;[(sys.stdout.write('\r\033[32m'+'█'*(i%40)+'\033[0m %d'%i),sys.stdout.flush(),time.sleep(0.1)) for i in range(80)]");
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
        roundtrip(r"C:\Program Files\app.exe", &["a b", r#"q "inner" q"#, "", r"C:\end\"]);
        roundtrip("/bin/echo", &["hello world"]);
    }
}
