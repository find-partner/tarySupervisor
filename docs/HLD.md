# 概要设计文档（HLD）

| 项目 | 内容 |
|------|------|
| 产品名称 | TraySupervisor（暂定名） |
| 版本 | v0.1（草案） |
| 日期 | 2026-10-02 |
| 依据 | [软件需求规格说明书（SRS）](./SRS.md) |

---

## 1. 架构总览

### 1.1 形态

**单进程 Tauri v2 桌面应用**，常驻托盘：

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
│  │ 模块    │  │  管理器    │  │  管道       │  │ 菜单构建/防抖 │  │
│  │TOML读写 │  │  状态机     │  │ 环形缓冲    │  └────▲─────────┘  │
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
3. **事件驱动刷新**：所有状态变化经内部广播通道（`tokio::sync::broadcast`）分发：托盘菜单重建、前端事件推送共用同一事件源。
4. **平台差异集中在两处**：`process` 平台分支（Unix/Windows 停止策略）与 `tray` 平台分支（菜单能力开关），其余代码平台无关。

---

## 2. 技术选型（已锁定基线）

| 领域 | 选型 | 说明 |
|------|------|------|
| 框架 | Tauri v2 | 托盘、窗口、IPC、打包 |
| 前端 | Leptos（或 Yew）+ Trunk | Rust→WASM，零 Node；`withGlobalTauri = true` |
| 托盘菜单 | `tauri::tray`（底层 tray-icon/muda） | 动态 `set_menu` 重建；`show_menu_on_left_click` 配置 |
| 异步运行时 | tokio | 进程管理、日志读取、事件广播 |
| 进程停止 | Unix：`libc::killpg`（setsid 进程组）；Windows：Job Object（`CREATE_SUSPENDED` + `AssignToJobObject`，手写 `windows-sys` 绑定，spike 已验证） | 兜底 `kill_tree` |
| PTY | `portable-pty`（WezTerm 生态） | 仅 PTY 启动模式加载 |
| 终端解析 | `vte` + 自研轻量渲染（只读） | 交互式终端 widget（egui_term 等）不在 MVP |
| 日志轮转 | `file-rotate` | `ContentLimit::Bytes` + `AppendCount` + `FileLimit::MaxFiles` + 可选压缩 |
| 正则 | `regex` | 保存期编译校验，失败即拒绝 |
| 变量替换 | `shellexpand::env_with_context` | 自定义上下文 = 本服务 env 表（有序）→ 系统环境 |
| 配置 | `toml` + `serde` + `notify`（热加载） | 单文件 `~/.traysupervisor/config.toml` |
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
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<EnvEntry>,              // 有序，支持条目间引用
    pub autostart: bool,
    pub autorestart: Autorestart,        // No | OnFailure | Always
    pub stop_timeout_secs: u64,          // 默认 10
    pub spawn_mode: SpawnMode,           // Pipe | Pty
    pub log: LogPolicy,
    pub matchers: Vec<MatcherConfig>,
}

pub struct EnvEntry {
    pub key: String,
    pub value: String,
    pub expand: bool,                    // false：字面量直传（默认）
}
```

职责：
- 加载 / 保存 TOML；**保存前校验**（命令可解析、正则可编译、env UTF-8、`${VAR}` 引用按顺序可解析且无环）。
- **展开函数** `resolve_env(&ServiceConfig) -> Result<BTreeMap<OsString, OsString>>`：按表顺序构建，`expand=true` 的条目以"当前已构建 map + 系统环境"为上下文执行替换；未定义变量返回错误。
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
  1. `resolve_env()` 得到环境变量表；
  2. 构建 `std::process::Command`（不经 shell）；
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
- PTY 输出同时进日志管道（时间戳、Matcher、环形缓冲同管道模式），即"终端视图"与"日志"共享数据源，仅在渲染层区分（是否走 vte 解析）。
- MVP 输入通道预留 `master_writer` 但不暴露 UI。

### 3.5 `app::tray` — 托盘菜单（Tauri 层）

- 订阅 `StateChanged` / `MatchHit` / `ConfigDelta` 事件 → **防抖合并（300ms）** → 全量重建菜单 → `tray.set_menu()`。
- 菜单 ID 编码：`svc:<id>:start|stop|restart|log|term|edit`、`match:<svc_id>:<hash>`、`app:settings|quit|start_all|stop_all`。
- 平台能力开关（编译期/运行时检测）：

| 能力 | Windows | macOS | Linux |
|------|---------|-------|-------|
| 单击弹菜单 | 左/右键均弹 | 左键弹（默认语义） | 由 DE 决定（AppIndicator 下点击即弹） |
| 菜单项图标（状态圆点用文本 ● 代替） | ✅ | ✅ | 视 DE，用文本兼容 |
| tooltip | ✅ | ✅ | 部分支持，不强依赖 |

- 启动时 Linux 检测：读取 `XDG_CURRENT_DESKTOP`；GNOME 下通过 `gnome-extensions info appindicatorsupport@gaudybort.com` 检测扩展，未启用则以通知 + 设置页横幅提示（SRS R2 缓解）。

### 3.6 前端（Leptos，WASM）

- 页面：服务列表 / 服务编辑（含 env KV 表、Matcher 表）/ 日志查看 / 终端只读视图 / 关于。
- 与后端交互仅通过 Tauri IPC（下节）；日志与终端视图均为 `listen("log-appended")` 增量渲染，终端视图额外接 `vte` WASM 解析（或后端解析后传渲染指令，见 §5 决策 D-3）。

---

## 4. IPC 接口设计

### 4.1 Command（前端 → 后端）

| Command | 参数 | 返回 |
|---------|------|------|
| `list_services` | — | `Vec<ServiceBrief>`（配置+运行态快照） |
| `upsert_service` | `ServiceConfig` | 校验结果 / 保存结果 |
| `delete_service` | `id` | 结果（运行中则报错要求先停止或确认） |
| `start_service` / `stop_service` / `restart_service` | `id` | 立即返回，结果经事件推送 |
| `get_log` | `id, since_index, limit` | `Vec<LogLine>`（历史回放，滚动加载用） |
| `open_match_url` | `svc_id, hash` | 校验 http/https 后 `tauri-plugin-opener` 打开 |
| `get_env_resolved_preview` | `ServiceConfig` | 展开后的 env 预览（编辑页"预览"按钮） |

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
| D-7 | 菜单全量重建 + 300ms 防抖，不做增量更新 | muda/DE 菜单增量 API 不一致，全量重建最可靠；防抖限频（FR-1.6） |

---

## 6. 目录结构（workspace 布局）

```
traysupervisor/
├── Cargo.toml                # workspace
├── crates/
│   └── core/                 # 纯 Rust 核心（无 tauri 依赖）
│       └── src/
│           ├── config/       # 加载/校验/展开/热载
│           ├── service/      # 状态机、spawn/stop、平台分支
│           ├── log/          # 合并器/环形缓冲/轮转/Matcher
│           ├── pty.rs
│           └── events.rs     # 事件与广播类型
├── src-tauri/                # Tauri 壳
│   └── src/
│       ├── main.rs / lib.rs
│       ├── commands.rs       # IPC
│       ├── tray.rs           # 菜单构建/防抖/平台开关
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
| Windows | Tauri NSIS/MSI | 确认 WebView2 Runtime 存在（Tauri 默认引导安装） |
| macOS | DMG | 公证（notarization）接入 CI |
| Linux | AppImage + deb | deb 依赖 webkit2gtk-4.1；AppImage 携带说明（GNOME 需扩展） |
| CI | GitHub Actions 三平台矩阵 | `trunk build` → `tauri build` |

---

## 8. 测试策略（概要）

1. **core 单元测试**：状态机转移表驱动测试；env 展开用例（嵌套引用、未定义、环、非 UTF-8）；Matcher 用例（URL 提取、捕获组模板、上限淘汰）。
2. **平台集成测试**：Unix 在 CI（Linux/macOS runner）验证 setsid+killpg 树杀；Windows runner 验证 Job Object 树杀（含孙进程，如 `cmd /c start python ...` 场景反例不出现——我们不提供 shell 包装）。
3. **日志轮转测试**：灌入超限输出验证分块数、总占用、压缩开关。
4. **手动冒烟矩阵**：Windows 11 / macOS 14 / Ubuntu 24.04（GNOME 含扩展、KDE），覆盖 SRS 全部 P0 验收标准。

---

## 9. 风险登记册

继承 [SRS §6](./SRS.md#6-风险登记册从技术调研带入)，设计层面补充：

| ID | 风险 | 设计应对 |
|----|------|---------|
| RD-1 | `vte` 后端渲染帧方案在 Windows 高频刷新场景性能未知 | 帧事件限频（≤30fps 合并）；冒烟阶段实测，必要时降级为纯文本流 |
| RD-2 | Tauri + Leptos 组合的 `withGlobalTauri` 与 wasm-bindgen 兼容性 | 立项首周 spike 验证（1 天工作量），失败备选 Yew |
| RD-3 | Linux 上 WebKitGTK 4.1 老发行版缺失 | 文档声明最低发行版（Debian 12 / Ubuntu 24.04）；AppImage 不捆绑 webkit（体积与兼容权衡） |
| RD-4 | 热加载 diff 应用与运行态竞争 | config 模块单写者（tokio mpsc 串行化），变更串行应用 |
