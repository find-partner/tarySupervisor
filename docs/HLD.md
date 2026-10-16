# 概要设计文档（HLD）

| 项目 | 内容 |
|------|------|
| 产品名称 | TraySupervisor（暂定名） |
| 版本 | v0.5（与 SRS v0.5 / CONFIG-SPEC v0.5 同步） |
| 日期 | 2026-10-05 |
| 依据 | [软件需求规格说明书（SRS）](./SRS.md) |
| 配套文档 | [配置文件规范（CONFIG-SPEC）](./CONFIG-SPEC.md)、[命令行工具规格（CLI-SPEC）](./CLI-SPEC.md) |

---

## 1. 架构总览

### 1.1 形态

**单进程 Tauri v2 桌面应用**，常驻托盘。另附 `traysvctl` 命令行工具（P1，[CLI-SPEC](./CLI-SPEC.md)），经本地 IPC（§3.7）控制常驻应用，不引入守护进程：

```
┌─────────────────────────────────────────────────────────────┐
│                    TraySupervisor 进程                        │
│                                                             │
│  ┌─────────────── 前端（Rust→WASM, Leptos/Yew）───────────┐  │
│  │  设置窗口 │ 日志窗口(每服务) │ 只读终端视图(PTY 模式)    │  │
│  └───────────────▲────────────────────▲───────────────────┘  │
│                  │ invoke(命令)        │ listen(事件)          │
│  ┌───────────────┴────────────────────┴───────────────────┐  │
│  │                   Tauri IPC 层 (commands.rs)            │  │
│  └──▲──────────────▲──────────────────▲──────────▲─────────┘  │
│     │              │                  │          │            │
│  ┌──┴─────┐  ┌─────┴─────┐  ┌─────────┴──┐  ┌────┴─────────┐  │
│  │ config │  │  service  │  │    log     │  │    tray      │  │
│  │ 模块    │  │  管理器    │  │  管道       │  │ 菜单构建/就地更新 │  │
│  │INI读写 │  │  状态机     │  │ 环形缓冲    │  └────▲─────────┘  │
│  │校验/热载 │  │  spawn/stop│  │ 轮转文件    │       │            │
│  └────────┘  └─────┬─────┘  │ Matcher    │  ┌────┴─────────┐  │
│                    │        └─────▲──────┘  │ tray-icon/   │  │
│              ┌─────┴─────┐        │         │ muda (Tauri) │  │
│              │  pty 模块  │────────┘         └──────────────┘  │
│              │portable-pty│                                    │
│              └─────┬─────┘                                     │
└────────────────────┼───────────────────────────────────────────┘
                     │ 子进程（setsid / Job Object）
              ┌──────┴──────┐
              │  服务进程树   │  stdout/stderr → 管道 或 PTY slave
              └─────────────┘
```

### 1.2 设计原则

1. **核心与 UI 解耦**：`config` / `service` / `log` / `pty` 为纯 Rust 模块（crate workspace 内的 `core` 成员），不依赖 `tauri`，可独立单测（NFR-8）。
2. **单进程托管**：服务是本进程的直接子进程（SRS C3）；生命周期与 Tauri 窗口无关，与整个应用进程绑定。
3. **事件驱动刷新**：所有状态变化经内部广播通道（`tokio::sync::broadcast`）分发：托盘菜单刷新、前端事件推送共用同一事件源。
4. **平台差异集中在两处**：`process` 平台分支（Unix/Windows 停止策略）与 `tray` 平台分支（菜单能力开关），其余代码平台无关。

---

## 2. 技术选型（已锁定基线）

| 领域 | 选型 | 说明 |
|------|------|------|
| 框架 | Tauri v2 | 托盘、窗口、IPC、打包 |
| 前端 | Leptos（或 Yew）+ Trunk | Rust→WASM，零 Node；`withGlobalTauri = true` |
| 托盘菜单 | `tauri::tray`（底层 tray-icon/muda） | 持久菜单就地更新（仅服务集合变化时 `set_menu` 重建）；`show_menu_on_left_click` 配置 |
| 异步运行时 | tokio | 进程管理、日志读取、事件广播 |
| 进程停止 | Unix：`libc::killpg`（setsid 进程组）；Windows：Job Object（`CREATE_SUSPENDED` + `AssignToJobObject`，手写 `windows-sys` 绑定，spike 已验证） | 兜底 `kill_tree` |
| PTY | `portable-pty`（WezTerm 生态） | 仅 PTY 启动模式加载 |
| 终端解析 | `vte` + 自研轻量渲染（只读） | 交互式终端 widget（egui_term 等）不在 MVP |
| 日志轮转 | `file-rotate` | `ContentLimit::Bytes` + `AppendCount` + `FileLimit::MaxFiles` + 可选压缩 |
| 正则 | `regex` | 保存期编译校验，失败即拒绝 |
| 变量替换 | `shellexpand::env_with_context` | 自定义上下文 = 本服务 env 表（有序）→ 系统环境 |
| 配置 | `rust-ini` + `notify`（热加载） | 单文件 `~/.traysupervisor/config.ini`，supervisord 风格 INI（D-8）；规范见 [CONFIG-SPEC](./CONFIG-SPEC.md) |
| CLI | `clap`（`crates/cli`） | `traysvctl`，supervisorctl 风格；经本地 IPC 控制常驻应用（D-9），见 [CLI-SPEC](./CLI-SPEC.md) |
| 本地 IPC | JSON-RPC 2.0 over UDS / Named Pipe | CLI 通道（§3.7 `app::ipc`），复用 §4 Command 语义 |
| 通知 | `tauri-plugin-notification` | P1 |

> 注意：托盘核心库 tray-icon 与 Tauri 的 `tauri::tray` 为同源封装；本项目统一使用 Tauri 层 API，避免双事件循环。若后续脱离 Tauri（如换 egui 路线），`core` 模块不需改动。

---

## 3. 模块设计

### 3.1 `core::config` — 配置模块

```rust
pub struct AppConfig {
    pub settings: GlobalSettings,        // 内存行数上限、通知开关等
    pub services: Vec<ServiceConfig>,    // 顺序即菜单顺序
}

pub struct ServiceConfig {
    pub id: String,                      // slug，唯一
    pub name: String,
    pub command: String,                 // 默认命令行（程序）
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<EnvEntry>,              // 有序，支持条目间引用与行级平台标签
    pub autostart: bool,
    pub autorestart: Autorestart,        // No | OnFailure | Always
    pub stop_timeout_secs: u64,          // 默认 10
    pub spawn_mode: SpawnMode,           // Pipe | Pty（默认捕获模式）
    pub term_interactive: bool,          // 终端视图可写（交互，默认 false；仅 Pty 有效，SRS FR-6.4）
    pub log: LogPolicy,
    pub matchers: Vec<MatcherConfig>,
    pub platform: BTreeMap<String, PlatformVariant>,  // 平台变体（FR-2.13），键 ∈ 注册表
}

pub struct EnvEntry {
    pub key: String,
    pub value: String,
    pub expand: bool,                    // false：字面量直传（默认）
    pub platform: Option<String>,        // 行级平台标签：None = 全部平台（FR-2.13）
}

/// 平台变体（FR-2.13 / DESIGN §4.2）：基础配置页字段的平台覆盖
pub struct PlatformVariant {
    pub command: String,                 // 空 = 未覆盖（args 同时为空）
    pub args: Vec<String>,
    pub directory: Option<PathBuf>,
    pub spawn_mode: Option<SpawnMode>,
    pub term_interactive: Option<bool>,
}

/// 平台注册表（DESIGN §4.1）：内置 windows/macos/linux + 全局声明自定义平台；
/// chain() = [current_platform] + [OS 家族] + [自动检测 OS]，变体沿链取第一个命中
pub struct PlatformRegistry { /* custom: Vec<PlatformDef>, current: Option<String> */ }
```

职责：
- 加载 / 保存 supervisord 风格 INI（编解码层 `rust-ini` → 语义模型，规范见 [CONFIG-SPEC](./CONFIG-SPEC.md)）；**保存前校验**（命令可解析、正则可编译、env UTF-8、`${VAR}` 引用按顺序可解析且无环）。
- **展开函数** `resolve_env(&ServiceConfig) -> Result<BTreeMap<OsString, OsString>>`：按表顺序构建，`expand=true` 的条目以"当前已构建 map + 系统环境"为上下文执行替换；未定义变量返回错误。
- **平台解析**（FR-2.13）：`resolve_for(&PlatformRegistry) -> ServiceConfig` 把平台变体折叠进基础字段并按行级标签过滤合并 env（同名键平台条目就地覆盖）；manager 在把配置交给监督任务前完成解析，原始配置（含全部变体）保留供存盘与编辑回显；`diff_configs` 对新旧配置各自按其解析链比较生效值——改其他平台的变体不触发本机重启。
- 热加载：`notify` 监听文件变更 → 重新加载 → 与运行态 diff → 产出 `ConfigDelta { added, removed, changed }`，由 service 管理器应用（未受影响的服务不重启）。

### 3.2 `core::service` — 进程管理器

状态机（对齐 Supervisor 语义）：

```
            ┌───────── autorestart=Always ─────────┐
            ▼                                      │
STOPPED ──start──► STARTING ──首行输出/宽限期──► RUNNING
   ▲                   │                        │
   │                  失败                     stop
   │                   ▼                        ▼
   └── 拉起失败 ◄── BACKOFF(1s,2s,4s…≤5次) ◄─ STOPPING ──优雅停止或超时强杀──► STOPPED
                        │  重试耗尽
                        ▼
                      FATAL ── 手动 start 可复活
RUNNING ──意外退出──► EXITED ──autorestart=OnFailure/Always──► BACKOFF
```

- **STARTING → RUNNING 判定**：进程存活超过 `startsecs`（默认 3 秒）或产生首行输出。
- 启动（spawn 流程）：
  1. `resolve_env()` 得到环境变量表；未显式配置 `PATH` 时注入登录 shell PATH 兜底（GUI 宿主 PATH 精简，见 `service::shellenv`，CONFIG-SPEC §3.5）；
  2. 构建 `std::process::Command`（不经 shell；`shell=true` 时经 `$SHELL -ilc` 包装以继承用户 shell 环境，CONFIG-SPEC §3.5）；
  3. Unix：`pre_exec(setsid)`（子进程自成会话/进程组）；Windows：`CREATE_SUSPENDED` 创建 → `AssignToJobObject`（Job 设 `KILL_ON_JOB_CLOSE`）→ `ResumeThread`；
  4. Pipe 模式：接管 stdout/stderr 两根管道，stdin 置空关闭；PTY 模式：`portable-pty` 打开 master/slave 并 spawn；
  5. 注册到运行态表 `HashMap<ServiceId, RunningService>`。
- 停止（stop 流程）：
  1. 置 STOPPING，启动超时计时；
  2. 优雅尝试：Unix `killpg(SIGTERM)`；Windows `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pgid)`（尽力而为，失败不阻断）；
  3. `tokio::time::timeout(stop_timeout_secs)` 等待退出；
  4. 超时强杀：Unix `killpg(SIGKILL)`；Windows `TerminateJobObject`；
  5. 回收（wait）→ 置 STOPPED，记录退出码与停止原因。
- 应用退出钩子：遍历运行态执行 stop 流程（detach 配置项跳过，P1）。
- **GUI 保存语义（SRS FR-2.10）**：`upsert_service` 保存文件后调用 `apply_saved_config()`——加载新配置、应用 added/removed 与日志/Matcher 即时更新，但 **changed 的运行中服务只换配置、不自动重启**；重启由用户在"配置已保存——需重启生效"提示中显式触发。文件 watcher 的外部编辑路径仍按 CONFIG-SPEC §8 自动重启；GUI 保存后 2s 内的 watcher 事件按"自写回声"抑制（`last_self_save` 时间戳）。

### 3.3 `core::log` — 日志管道

```
stdout pipe ─┐
             ├─► 合并器（行切分 + 时间戳 + 流标识）─┬─► 环形缓冲 VecDeque<LogLine>（≤N 行）
stderr pipe ─┘                                    ├─► file-rotate sink（按配置）
             PTY master（PTY 模式单流）────────────┘    └─► Matcher 引擎（流式逐行）
                                                        └─► broadcast: LogAppended 事件
```

- `LogLine { ts, stream: Out|Err, text }`；单行超 64KB 截断（NFR-6）。
- Matcher 引擎：对每条新行执行该服务全部已编译 regex；命中提取捕获组 → 模板渲染 → `MatchHit { key: hash, label, url }` 发事件；托盘与前端订阅。
- 文件轮转：`FileRotate::new(path, AppendCount::new(max_files), ContentLimit::Bytes(max_bytes), compress)`；应用重启后追加写。
- 解码：字节流按 UTF-8 增量解码（`String::from_utf8_lossy` 分块处理），非法字节替换显示。

### 3.4 `core::pty` — 伪终端

- 封装 `portable-pty`：`open_pty(size) → (master_reader, master_writer, child)`。
- **PTY master 生命周期 = 服务生命周期**，与任何视图窗口无关；视图（前端）只是订阅 `LogAppended` 的一个客户端（FR-6.2）。
- **输入通道（SRS FR-6.4，已启用）**：`RunCtx.pty_writer` 挂到 `SvcCtx`（`Arc<Mutex<Option<Box<dyn Write + Send>>>>`），spawn 成功时写入、进程退出时清空；`ServiceManager::write_stdin(id, bytes)` 加锁 `write_all + flush`（None = 未运行或非 PTY）。前端在 `term_interactive` 服务的终端窗口监听键盘/粘贴事件按字节转发（行编辑交给子进程行规程）。
- PTY 输出同时进日志管道（时间戳、Matcher、环形缓冲同管道模式），即"终端视图"与"日志"共享数据源，仅在渲染层区分（是否走 vte 解析）。
- MVP 输入通道预留 `master_writer` 但不暴露 UI。

### 3.5 `app::tray` — 托盘菜单（Tauri 层）

- 订阅 `StateChanged` / `MatchHit` / `ConfigDelta` 事件，按类别分流（SRS FR-1.6）：
  - **StateChanged** → 持久菜单项**就地更新**：状态行 `set_text` + `set_icon`（色点）、启动/停止/重启 `set_enabled`、tooltip 更新；
  - **MatchHit** → 该服务子菜单内**移除旧快捷项、追加新快捷项**（哈希列表未变化时跳过）；
  - **ConfigDelta（服务集合变化）** → 才整体重建菜单并 `tray.set_menu()`。
  - 理由：菜单展开期间调用 `set_menu()` 替换菜单对象会被系统直接关闭（"菜单消失"问题）；就地更新（muda `set_text/set_icon/set_enabled`、`Submenu::append/remove`）不触发关闭，未展开项最迟下次展开呈现新值。
- 菜单 ID 编码：`svc:<id>:start|stop|restart|log|term|edit`、`match:<svc_id>:<hash>`、`app:settings|quit|start_all|stop_all`。
- 数据源与设置页一致：菜单构建/就地更新统一读 `ServiceManager::list()` 快照（SRS FR-1.10）。
- 平台能力开关（编译期/运行时检测）：

| 能力 | Windows | macOS | Linux |
|------|---------|-------|-------|
| 单击弹菜单 | 左/右键均弹 | 左键弹（默认语义） | 由 DE 决定（AppIndicator 下点击即弹） |
| 菜单项图标（状态圆点用文本 ● 代替） | ✅ | ✅ | 视 DE，用文本兼容 |
| tooltip | ✅ | ✅ | 部分支持，不强依赖 |

- 启动时 Linux 检测：读取 `XDG_CURRENT_DESKTOP`；GNOME 下通过 `gnome-extensions info appindicatorsupport@gaudybort.com` 检测扩展，未启用则以通知 + 设置页横幅提示（SRS R2 缓解）。

### 3.6 前端（Leptos，WASM）

- 页面：服务列表 / 服务编辑（含 env KV 表、Matcher 表）/ 日志查看 / 终端视图 / 关于。
- 与后端交互仅通过 Tauri IPC（下节）；日志与终端视图均为 `listen("log-appended")` 增量渲染，终端视图额外接 `vte` WASM 解析（或后端解析后传渲染指令，见 §5 决策 D-3）。
- **共享服务数据源（`store`，SRS FR-1.10）**：每个窗口内以全局信号持有 `list_services` 快照，订阅 `service-state-changed` 就地更新对应条目、`config-changed`/`config-invalid` 全量刷新；设置页服务列表与编辑页操作栏（FR-2.11）共用该数据源，保证与托盘菜单状态一致。
- **编辑页服务操作栏（SRS FR-2.11）**：编辑模式标签栏上方提供 状态显示（色点+状态+PID）、启动/停止/重启（按状态启用/禁用）、查看日志、终端视图；动作分别调用 `start/stop/restart_service` 与 `open_log_window`/`open_term_window`。
- **设置页三态（SRS FR-2.7~2.9）**：
  - **空态**：未选中且未新建 → 编辑区显示产品简介 + "请新建配置"引导，无表单；
  - **编辑模式**：按标签页分组（基本设置 / 环境变量 / 日志 / 快捷项），每页独立"保存本页"（以整份配置快照合并提交 upsert），未保存变更在标签上标记"未保存"，切换页面不隐式保存；
  - **新建向导**：① 基本设置 → ② 环境变量 → ③ 日志 → ④ 快捷项，可上一步/跳过，最后"保存并完成"；ID 与命令行非空方可离开第①步。
- **保存生效提示（SRS FR-2.10）**：保存成功后若服务 RUNNING/STARTING → 编辑区横幅"需重启生效" + "立即重启"按钮（`restart_service`）。
- **列表/编辑回显一致性**：`list_services` 返回的 brief 以**嵌套** `config` 对象反序列化（前端 DTO 与 core `ServiceBrief` 形状一一对应；禁用 flatten —— 曾因 flatten 误配导致列表无名、编辑空白，见 D-14）。
- **终端读写（SRS FR-6.4/6.5）**：`term_interactive && Pty` 的服务在终端窗口提供"只读/可写"开关与键盘/粘贴输入转发（`write_stdin`），控制序列按 ANSI 字节直传。
- **终端渲染细节（SRS FR-6.6）**：屏幕帧每行渲染后补真实换行（`white-space: pre` 依赖 `\n` 文本节点分行）；服务每次启动时后端清空 vte 栅格（会话隔离，PTY 只保留当前会话）。

### 3.7 `app::ipc` — 本地 IPC 服务（CLI 接入，Tauri 层）

- 常驻应用启动时创建端点：Unix `~/.traysupervisor/traysv.sock`（权限 0600）/ Windows `\\.\pipe\traysupervisor-ctl`（ACL 限当前用户）；应用退出时清理。
- 协议：JSON-RPC 2.0，NDJSON 帧；方法集 = §4.1 Command 的复用 + `reload_config(mode: reread|update)`、`shutdown`、`version`。无服务端主动推送；`tail -f` 以长连接逐行写出 `LogAppended` 增量（唯一流式例外）。
- 并发：允许多连接；控制命令经与热加载相同的**单写者通道**串行应用（RD-4）；`shutdown` 触发 FR-1.8 退出流程。
- 安全：命令白名单固定，不提供任意命令执行；`crates/cli` 仅依赖 core 与 IPC 客户端，不含 tauri。

---

## 4. IPC 接口设计

### 4.1 Command（前端 → 后端）

| Command | 参数 | 返回 |
|---------|------|------|
| `list_services` | — | `Vec<ServiceBrief>`（配置+运行态快照） |
| `upsert_service` | `ServiceConfig` | 校验结果 / 保存结果 |
| `delete_service` | `id` | 结果（运行中则报错要求先停止或确认） |
| `start_service` / `stop_service` / `restart_service` | `id` | 立即返回，结果经事件推送 |
| `open_log_window` / `open_term_window` | `id` | 打开（或聚焦）该服务的日志窗口 / 终端视图窗口（托盘菜单与编辑页操作栏共用入口） |
| `get_log` | `id, since_index, limit` | `Vec<LogLine>`（历史回放，滚动加载用） |
| `open_match_url` | `svc_id, hash` | 校验 http/https 后 `tauri-plugin-opener` 打开 |
| `get_env_resolved_preview` | `ServiceConfig` | 展开后的 env 预览（按当前平台解析链过滤合并后展开；编辑页"预览"按钮） |
| `parse_pasted_line` | `line: String` | `PastedLine { env: Vec<EnvEntry>, command_line: String }`（FR-2.12 粘贴解析，core 纯函数） |
| `get_platform_info` | — | `PlatformInfo { native_os, platforms: Vec<PlatformDef>, current, chain }`（平台下拉选项/本机角标/解析链提示，FR-2.13） |
| `reload_config` | `mode: reread \| update` | `ConfigDelta`（reread 仅报告差异；update 应用差异，语义见 [CLI-SPEC](./CLI-SPEC.md)） |
| `shutdown` | — | 触发退出流程（FR-1.8：逐个停止运行中服务后退出） |
| `write_stdin` | `id, data: Vec<u8>` | 转发至 PTY master（`term_interactive` 服务；非 Pty/未运行报错） |

> 以上 Command 同时经 §3.7 本地 IPC 以 JSON-RPC 暴露给 `traysvctl`（方法名映射见 [CLI-SPEC](./CLI-SPEC.md) §2）。

### 4.2 Event（后端 → 前端，均来自 broadcast 总线）

| Event | 载荷 | 消费方 |
|-------|------|--------|
| `service-state-changed` | `{ id, state, pid?, exit_code?, reason? }` | 前端状态、托盘 |
| `log-appended` | `{ id, lines: Vec<LogLine> }` | 日志窗口、终端视图 |
| `matcher-hit` | `{ svc_id, items: Vec<MatchHit> }` | 托盘 |
| `config-changed` | `{ delta }` | 前端列表、托盘 |

---

## 5. 关键设计决策记录

| ID | 决策 | 理由 |
|----|------|------|
| D-1 | 不做左右键区分，单击弹菜单（三平台一致） | macOS NSStatusItem 无原生右键语义，hack 不可靠（SRS R1） |
| D-2 | 单进程托管，不做守护进程 | MVP 复杂度控制；"关闭终端不影响服务"由 PTY 归属解决，无需守护进程 |
| D-3 | 只读终端渲染：后端 `vte` 解析输出"渲染帧/差异"事件，前端仅绘制 | 避免在 WASM 侧维护终端状态机；前端轻；二期若上交互终端再评估 egui_term/自研 |
| D-4 | env 替换只支持 `${VAR}`、按表顺序解析、不支持转义、未定义报错 | 语义最小闭环，规避转义与双语法复杂度（SRS R7） |
| D-5 | 停止策略：TERM/CTRL_BREAK → 超时 → 强杀；Windows 全程 Job Object 兜底 | Windows 无信号，树杀必须靠 Job（SRS R3） |
| D-6 | 日志双 sink（内存必选 + 文件可选），Matcher 在合并器之后 | 关闭文件日志仍可查看；Matcher 与存储解耦 |
| D-7 | 菜单**持久化 + 菜单项就地更新**（状态/启用位/图标 `set_*`，Matcher 项子菜单内增删），仅服务集合变化时全量重建 | 菜单展开期间 `set_menu()` 会被系统直接关闭（"菜单消失"缺陷）；就地更新不关闭菜单且天然免防抖（FR-1.6 由"防抖重建"改为"就地更新"） |
| D-8 | 配置文件采用 supervisord 相似风格 INI（`[program:<id>]` 分节、字段名对齐、特有功能同风格扩展） | 复用 Supervisor 用户习惯与文档资产；INI 足以表达现有语义模型（environment 有序对 + matcher 编号键）；规范见 [CONFIG-SPEC](./CONFIG-SPEC.md) |
| D-9 | 新增 `traysvctl` CLI 与本地 IPC 通道（UDS/Named Pipe，JSON-RPC），CLI 与 GUI 共用 `crates/core` | 提供脚本化 / SSH 会话的管理入口；不引入守护进程，CLI 仅是常驻应用的客户端（C3、D-2 不变）；规范见 [CLI-SPEC](./CLI-SPEC.md) |
| D-10 | GUI 保存**不自动重启**运行中服务：`apply_saved_config` 只换配置，前端提示"需重启生效"+ 显式重启按钮；文件 watcher 的外部编辑路径仍自动重启（CONFIG-SPEC §8），GUI 自写回声 2s 抑制 | 编辑期反复保存不应打断服务；把"何时生效"的决定权交给用户（SRS FR-2.10） |
| D-11 | 交互终端 opt-in（`term_interactive` 默认关、仅 Pty）：输入按字节直传 PTY master，行编辑交给子进程行规程，窗口内提供只读/可写运行时开关 | 误输入风险默认关闭；不做本地行缓冲，保持与真实终端一致的最小实现（SRS FR-6.4/6.5）；窗口 resize 转发留二期 |
| D-12 | 设置页标签页"每页独立保存"= 以整份配置快照合并提交（单次 upsert_service） | 后端无需按页拆分的保存 API；校验（FR-2.4）天然作用于整份配置，避免半成品配置落盘 |
| D-14 | 前端 DTO 与 core 序列化形状**逐字段对齐、禁用 flatten**（`list_services` 的 brief.config 为嵌套对象） | serde flatten 对嵌套对象静默降级为默认值（曾导致列表无名、编辑空白且无任何报错），嵌套结构显式可查、编译期字段拼写即校验 |
| D-15 | 托盘菜单、设置页列表、编辑页操作栏共用同一数据源与事件（`ServiceManager::list()` 快照 + `service-state-changed`） | 各自独立拉取/刷新曾导致设置页状态与托盘不一致；单一数据源 + 单一事件流保证三处一致（FR-1.10 / FR-2.11） |
| D-16 | 日志大小以**带单位可读值**配置与展示（`10MB`/`512KB`/`1GB`，兼容纯字节），core 统一提供 parse/format，内存模型仍为字节 | 裸字节数（10485760）不可读易错；与 supervisord 的数值后缀习惯一致；解析/格式化收敛在 core 避免前端重复实现（FR-2.2） |
| D-17 | PTY 终端视图按**会话**隔离：服务每次启动时 vte 栅格复位清屏，日志窗口/文件仍跨会话保留 | 终端 = "当下"（进度条、REPL 等重绘场景依赖干净栅格）；日志 = 历史（带时间戳可回溯）；两者语义分离（FR-6.6） |

---

## 6. 目录结构（workspace 布局）

```
traysupervisor/
├── Cargo.toml                # workspace
├── crates/
│   ├── core/                 # 纯 Rust 核心（无 tauri 依赖）
│   │   └── src/
│   │       ├── config/       # INI 解析/校验/展开/热载（CONFIG-SPEC）
│   │       ├── service/      # 状态机、spawn/stop、平台分支
│   │       ├── log/          # 合并器/环形缓冲/轮转/Matcher
│   │       ├── pty.rs
│   │       └── events.rs     # 事件与广播类型
│   └── cli/                  # traysvctl（clap + IPC 客户端，无 tauri 依赖）
│       └── src/main.rs
├── src-tauri/                # Tauri 壳
│   └── src/
│       ├── main.rs / lib.rs
│       ├── commands.rs       # IPC（前端）
│       ├── ipc.rs            # 本地 IPC 服务（traysvctl 接入，§3.7）
│       ├── tray.rs           # 菜单构建/就地更新/平台开关
│       └── bridge.rs         # core 事件 → Tauri emit
├── src/                      # Leptos 前端（Trunk 构建）
│   ├── app.rs  pages/  components/
│   └── index.html
├── Trunk.toml
├── tauri.conf.json           # withGlobalTauri: true; frontendDist: ../dist
└── docs/
```

---

## 7. 打包与分发

| 平台 | 方式 | 备注 |
|------|------|------|
| Windows | Tauri NSIS/MSI | 确认 WebView2 Runtime 存在（Tauri 默认引导安装）；产物需代码签名（NFR-10：SAC 强制模式拦截未签名 exe，spike 2026-10-03 验证记录） |
| macOS | DMG | 公证（notarization）接入 CI |
| Linux | AppImage + deb | deb 依赖 webkit2gtk-4.1；AppImage 携带说明（GNOME 需扩展） |
| CLI | `traysvctl` 随各平台安装包附带 | 另提供三平台独立压缩包（[CLI-SPEC](./CLI-SPEC.md) §8） |
| CI | GitHub Actions 三平台矩阵 | `trunk build` → `tauri build` |

---

## 8. 测试策略（概要）

1. **core 单元测试**：状态机转移表驱动测试；env 展开用例（嵌套引用、未定义、环、非 UTF-8）；Matcher 用例（URL 提取、捕获组模板、上限淘汰）。
2. **平台集成测试**：Unix 在 CI（Linux/macOS runner）验证 setsid+killpg 树杀；Windows runner 验证 Job Object 树杀（含孙进程，如 `cmd /c start python ...` 场景反例不出现——我们不提供 shell 包装）。
3. **日志轮转测试**：灌入超限输出验证分块数、总占用、压缩开关。
4. **CLI 集成测试**：`traysvctl` 对真实 IPC 的 status/start/stop/restart/退出码矩阵（含"应用未运行"= 3）；`reread`/`update` 差异语义（CONFIG-SPEC §8）。
5. **交互终端测试**：PTY 服务跑 `cat`/`python -i`，注入 `write_stdin` 验证回显、Ctrl-C 中断、方向键转义序列（FR-6.4）；`apply_saved_config` 保存后 RUNNING 服务**不重启**、重启后新配置生效（FR-2.10）。
6. **手动冒烟矩阵**：Windows 11 / macOS 14 / Ubuntu 24.04（GNOME 含扩展、KDE），覆盖 SRS 全部 P0 验收标准。

---

## 9. 风险登记册

继承 [SRS §6](./SRS.md#6-风险登记册从技术调研带入)，设计层面补充：

| ID | 风险 | 设计应对 |
|----|------|---------|
| RD-1 | `vte` 后端渲染帧方案在 Windows 高频刷新场景性能未知 | 帧事件限频（≤30fps 合并）；冒烟阶段实测，必要时降级为纯文本流 |
| RD-2 | Tauri + Leptos 组合的 `withGlobalTauri` 与 wasm-bindgen 兼容性 | 立项首周 spike 验证（1 天工作量），失败备选 Yew |
| RD-3 | Linux 上 WebKitGTK 4.1 老发行版缺失 | 文档声明最低发行版（Debian 12 / Ubuntu 24.04）；AppImage 不捆绑 webkit（体积与兼容权衡） |
| RD-4 | 热加载 diff 应用与运行态竞争 | config 模块单写者（tokio mpsc 串行化），变更串行应用 |
