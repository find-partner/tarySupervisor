## 修复目标

1. **焦点丢失 bug**：快捷项表和环境变量表每敲一个字符整表 DOM 被卸载重建（整个 tbody 是读整个 `form` 信号的动态子树）。改用 Leptos 0.6 `<For>` keyed 行——已从 leptos_dom 0.6.15 `each.rs apply_diff` 源码确认：同 key 行的 DOM 和响应式作用域完整保留，只有增/删/移动才动 DOM。key 必须用稳定 uid（不能用 pattern 内容或索引）。
2. **三段式快捷项**：`正则 + 动作 + 参数模版`。动作预定义为下拉枚举（用户可发现），动作与文案模版分离。
3. **动作执行**：托盘事件流里对新出现的命中（hash diff）执行动作。

## 已定的产品决策（可后续调整）

- **模版统一是文案**：按钮文字/通知正文；URL 恒从捕获组自动探测（任一捕获组以 http(s) 开头，现状规则）。
- **动作枚举**（INI/JSON 值）：`open_button` 打开按钮（= 现状，默认值）/ `open_url` 直接打开页面（命中即自动打开）/ `notify` 弹出通知。
- 所有命中仍在托盘菜单显示（可再次打开），自动打开与通知是附加行为；无 URL 的项菜单中仍禁用。
- 旧配置无 action 键 → `open_button`，行为完全不变；新建服务内置 URL 规则也是 `open_button`。
- 仅"新出现"的 hash 触发一次动作（LRU 复活不重触发；服务重启清空后重新命中会再次触发）。

## 改动清单

### core（crates/core）
- `src/config/mod.rs:403-426`：新增 `MatcherAction` 枚举（`snake_case` serde，Default=OpenButton，实现 `FromStr`）；`MatcherConfig` 加 `#[serde(default)] pub action: MatcherAction`。
- `src/config/ini/mod.rs:87-91`：写出 `matcher.N.action=`。
- `src/config/ini/program.rs:199-269`：`parse_matchers` 接受 `action` 键（非法值报错，缺省 OpenButton）。
- `src/matcher.rs`：`CompiledMatcher` 带 action；`short_hash` 输入加 action（改动作会让旧项淘汰、新项生效）；`feed_line` 组装 `MatchItem` 带 action；测试更新 + 新增 action 用例。
- `src/events.rs:111-118`：`MatchItem` 加 `pub action: MatcherAction`。

### src-tauri
- `Cargo.toml` + `lib.rs` + `capabilities/default.json`：引入 `tauri-plugin-notification = "2"`、注册插件、加 `notification:default` 权限（通知动作需要）。
- `commands.rs`（MatcherIn ~49、转换 ~229）：`action: String`（serde default），经 `FromStr` 转 core 枚举，非法回退 OpenButton。
- `tray.rs:76-83` 事件循环：`HashMap<svc_id, HashSet<hash>>` 记录已见 hash；新命中的 `open_url` → `is_safe_url` 校验后 opener 打开；`notify` → 系统通知（标题=服务显示名，正文=模版渲染）；`apply_matchers` 菜单逻辑不变。

### 前端（src/）
- `dto.rs`：`MatcherConfig` 加 `action: String`（default "open_button"）；`MatchItem` 加 `action: String`。
- `form.rs`：新增前端专用行模型 `MatcherRow { uid, pattern, action, template, max_items: String }`（max_items 用 String 与 startsecs 等现有约定一致，避免数字输入框清空即跳回 5）与 `EnvRow { uid, entry }`；`FormState.env/matchers` 换成行模型；`form_from_brief` / `blank_form` / `input_from_form` 适配（uid 用 AtomicU64 计数器）；`merged_for_save` 无需改（整行 clone）。
- `panels.rs:131-190` matcher_panel 重写：`<For each key=uid>` 渲染行；行内输入框按 uid 读写 form；新增"动作"下拉（select 用构建期 `selected` + `on:change`，同 basic.rs platform_select 的已验证模式）；删除按钮按 uid retain；更新列头与提示文案（说明三种动作与 URL 探测规则）。
- `basic.rs:417-528` env_table 同样 `<For>` 化；`platform_select` 从按索引改为按 uid 读写。

### 文档 & 验证
- `docs/CONFIG-SPEC.md` §5：补 `matcher.N.action` 键说明与示例。
- `cargo test -p traysupervisor-core`（matcher/INI 用例更新 + 新增 action 往返、缺省兼容用例）；`cargo check --workspace`；`trunk build`（wasm 编译检查）。

## 已知行为说明

- "直接打开页面"在服务重启后（快捷项清空重匹配）会再次自动打开——这是"命中即打开"的语义。
- macOS 上通知首次触发依赖系统授权，未授权时静默失败（仅记日志），Windows/Linux 正常。
- 无关的未跟踪文件 `crates/core/tests/reload_env_repro.rs` 不动。