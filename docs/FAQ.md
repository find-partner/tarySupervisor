# 常见问题清单（FAQ）

记录开发/联调中踩过的坑：每条给出**现象 → 根因 → 修复**，并沉淀可复用的排查规则。
遇到"疑似同类"的新问题，先对照本文档再动手；新坑修完后补录到此（含 commit 号）。
运行期报错可先看页面底部红横幅（index.html 兜底错误上报：JS 错误 / 资源加载失败 /
未处理 Promise 拒绝都会显示在这里——托盘应用的 WebView 控制台平时不可见）。

---

## Q1 删除服务报 "Promise拒绝: Command plugin:dialog|confirm not allowed by ACL"，删除/确认类按钮无效

**现象**：设置页点"删除"后弹横幅报 ACL 错误，或（修 ACL 后）确认框正常弹出但点了
"好"仍然什么都不发生、无任何报错。

**根因**（两层，都要满足才能正常工作）：

1. **ACL 层**：`tauri-plugin-dialog` 注入 init 脚本，把 webview 里的全局
   `window.confirm` / `window.alert` 覆盖为 async 函数，内部分别走
   `plugin:dialog|confirm` / `plugin:dialog|message` 命令。而该插件的
   `dialog:default` 权限集**只含 `allow-message` / `allow-save` / `allow-open`**，
   不含 `allow-confirm` / `allow-ask`——`dialog:default` ≠ 全量。未授权时命令被
   Tauri ACL 拒绝，`window.confirm` 返回的 Promise 以
   `Command plugin:dialog|confirm not allowed by ACL` 拒绝，页面红横幅即报
   "Promise拒绝: …"。
2. **语义层**：覆盖后的 `window.confirm` 返回 **Promise\<bool\>**，不能再用
   `web_sys::Window::confirm_with_message()`（同步 bool 绑定）取结果——glue 把
   JS 返回值原样传回 wasm ABI（i32），Promise 对象经 ToInt32 强转恒为 0，即
   **无论用户点什么、ACL 是否放行，Rust 侧永远拿到 `false`（= 取消）**。这就是
   "放开了权限还是删不掉、且无报错"的来源。

**修复**：

- ACL：`src-tauri/capabilities/default.json` 显式增加 `dialog:allow-confirm`
  （9d95452）。`window.alert` 走的 `allow-message` 已在 `dialog:default` 内；
  `window.prompt` 未被插件覆盖（2.8.x 的 init 脚本只覆盖 alert/confirm），无需
  `allow-ask`。
- 语义：前端统一经 `src/bridge.rs` 的 `confirm(msg)`（js_sys 取函数 → 结果为
  Promise 则 await，否则按同步 bool 处理），禁止直接用 `confirm_with_message`
  （本次修复）。

**同类排查规则**（前端触达插件命令的三条路径，都要核对 ACL）：

| 路径 | 例子 | 需要的权限 |
|---|---|---|
| 被插件覆盖的 window 全局 | `confirm` / `alert` | `dialog:allow-confirm` / `dialog:default`(message) |
| `__TAURI__` 模块 API | `__TAURI__.core.invoke`、`__TAURI__.event.listen` | `core:default`、`core:event:default`（已覆盖 listen/emit） |
| 显式 invoke 插件命令 | `invoke("plugin:xxx\|yyy")` | 对应插件 `allow-*` |

补充约定：

- app 自定义命令（`list_services`、`delete_service` 等）不走 ACL；Rust 侧直接调
  插件 API（如 `open_log_dir` → opener）也不受 ACL 限制。
- `capabilities/default.json` 的 `windows: ["*"]` 已覆盖动态创建的日志/终端窗口，
  新增窗口无需单独授权；新增**插件命令调用**才需要回头核对本表。
- `navigator.clipboard`（日志页复制）是 Web API，不经 Tauri ACL，只需注意
  WKWebView 的用户手势要求（按钮点击内调用即可）。

---

## Q2 设置/日志/终端窗口白屏：DOM 完整、wasm 加载正常、无 JS 错误

**现象**：窗口白屏；wasm 资源 HTTP 200、WASM 启动正常、控制台无错误，
body 里能看到完整 DOM 但不可见。

**根因**：Leptos 用 `mount_to_body` 挂载时，UI 成为 `index.html` 中 `#app`
（height:100%，占满整屏）的**后继兄弟节点**，页面根元素（`.settings` /
`.logwin` / `.termwin`，height:100%）被推出视口——DOM 存在但全部不可见。

**修复**：`mount_to(#app)`（f97bb57，保留 `mount_to_body` 兜底），恢复
`#app` 预期用途与高度链。

**排查规则**：白屏类问题先查"DOM 在不在 → 高度链断没断 → 挂载点是谁"，
再查资源加载与 JS 报错（本项目的 DOM/报错可见性由 index.html 兜底上报保障）。
