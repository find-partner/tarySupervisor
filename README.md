# TraySupervisor

跨平台（Windows / macOS / Linux）**系统托盘后台服务管理工具**：把自定义命令行服务注册进来，通过托盘菜单启动 / 停止 / 重启、查看日志、观察只读终端视图，并从日志中用正则提取快捷链接（如"🌐 打开 http://localhost:3000"）。对标 Python Supervisor 的进程语义（状态机 / autostart / autorestart / 日志轮转），但以桌面托盘 GUI 形态呈现，且明确支持 Windows。

技术栈（锁定）：**Tauri v2 + Rust 后端 + Leptos（Rust→WASM，经 Trunk 构建），全程无 Node.js**。

- 需求：[docs/SRS.md](docs/SRS.md) ｜ 概要设计：[docs/HLD.md](docs/HLD.md)
- 配置规范：[docs/CONFIG-SPEC.md](docs/CONFIG-SPEC.md)（示例：[config.example.ini](config.example.ini)）
- CLI 规格：[docs/CLI-SPEC.md](docs/CLI-SPEC.md)（P1，尚未实现）
- 开发规约：[docs/DEV-CONVENTIONS.md](docs/DEV-CONVENTIONS.md)（版本兼容性策略 / IPC 契约约束）
- 技术验证记录：[spikes/README.md](spikes/README.md)

---

## MVP（P0）实现状态

| SRS 需求 | 状态 | 实现位置 |
|----------|------|----------|
| FR-1 托盘与主菜单 | ✅ | `src-tauri/src/tray.rs`（持久菜单 + 就地更新、稳定菜单 ID、状态圆点、tooltip、退出确认、单实例） |
| FR-2 服务配置管理 | ✅ | `crates/core/src/config/`（supervisord 风格 INI、§3.4 切分、`${VAR}` 展开、逐项校验）+ `src-tauri/src/commands.rs`（保存/删除/预览）+ 前端设置页 |
| FR-2.6 配置热加载 | ✅ | `crates/core/src/watcher.rs`（notify 500ms 去抖 → diff → added/removed/changed 串行应用，校验失败沿用上次配置并提示） |
| FR-3 生命周期状态机 | ✅ | `crates/core/src/service/supervisor.rs`（STARTING→RUNNING 判定、BACKOFF 指数退避 ≤startretries、FATAL、EXITED；优雅停止→超时强杀） |
| FR-3 平台停止策略 | ✅ | `crates/core/src/service/platform.rs`（Unix：`setsid`+`killpg`；Windows：`CREATE_SUSPENDED`+Job Object(KILL_ON_JOB_CLOSE)+`TerminateJobObject`，spike 已验证时序） |
| FR-4 日志 | ✅ | `crates/core/src/log/`（环形缓冲默认 2000 行、file-rotate 按字节轮转 `.1 .2 …`、gzip、64KB 行截断、UTF-8 lossy）+ 日志窗口（自动滚动/暂停/复制/清空/打开目录） |
| FR-5 Matcher 快捷项 | ✅ | `crates/core/src/matcher.rs`（流式匹配、结果哈希去重、每条 max_items LRU、内置 URL 预设规则（新建时预填）、http/https 校验；服务重启清空） |
| FR-6 终端视图 | ✅ | `crates/core/src/term.rs`（后端 vte 解析：SGR 颜色/加粗/反显、CUP、ED/EL、CR 进度条重写等）+ 终端窗口（黑底，关窗不影响服务）；管道模式退化为纯文本流 |
| FR-6.4/6.5 交互（读写）终端 | ✅ | 每服务 `term_interactive` 配置开关（仅 Pty）；键盘/粘贴输入按 ANSI 字节转发 `write_stdin`→PTY master，行编辑交给子进程行规程；窗口内只读/可写开关 |
| FR-2A 设置页交互 | ✅ | 空态引导、四标签页分组（基本/环境变量/日志/快捷项）每页独立保存、新建 step-by-step 向导、保存后"需重启生效"提示 + 立即重启（FR-2.7~2.10） |
| FR-3.7 退出清理 | ✅ | SIGTERM/SIGINT 处理 + `RunEvent::ExitRequested` → `shutdown_all()`（本机冒烟验证：托盘应用退出后托管子进程无残留） |
| FR-9 traysvctl CLI（P1） | ⬜ 未实现 | 属首个正式版范围，规格已定（CLI-SPEC） |
| FR-7 通知 / FR-8 开机自启（P1/P2） | ⬜ 未实现 | 模型字段已预留（`notify_on_failure`） |

### 测试

系统测试计划与用例记录见 [docs/SYSTEM-TEST.md](docs/SYSTEM-TEST.md)（层级矩阵 / 脚本化用例 / 手动 GUI 用例 / 三平台记录表 / 偏差豁免）。

- **core 单元测试 63 个**：命令行切分/反切分 round-trip（含 SYSTEM-TEST 示例命令的保真锁定）、env 解析与 `${VAR}` 展开（含未定义报错）、INI 解析（未知键/编号断档/枚举非法即拒/BOM 容忍）、保存-加载 round-trip（0600 权限）、环形缓冲淘汰、轮转+压缩、64KB 截断、Matcher LRU/去重/URL 提取、vte 序列（SGR/CUP/ED/CR 重绘/滚动/反显）。
- **core 集成测试 8 个**（`crates/core/tests/service_tests.rs`，真实进程）：启动/停止无残留进程、意外退出→EXITED、`kill -9` 后 always 自动重启（新 PID）、spawn 失败→BACKOFF→FATAL、env 展开注入子进程、真实输出命中 URL Matcher、PTY 捕获+终端快照、热加载 diff 应用。
- **脚本化系统冒烟**（自动备份/恢复用户配置，断言内嵌）：
  - `scripts/smoke-unix.sh`（macOS/Linux）：autostart 拉起+父子关系、SIGTERM 退出清理、日志轮转份数/占用、热加载 applied/沿用、0600 权限、菜单就地更新（状态扰动零重建）——本机 8/8 断言通过。
  - `scripts/smoke-windows.ps1`（Windows）：cmd→ping 孙进程整树终止、硬杀后 KILL_ON_JOB_CLOSE 兜底、热加载。
- **CI**（`.github/workflows/ci.yml`）：三平台矩阵——core 测试（Windows 跑单元以编译回归 Job Object 平台分支）+ `cargo check --workspace --all-targets` + trunk WASM 构建。

---

## 开发与运行

前置：Rust（含 `wasm32-unknown-unknown` target）、`cargo install trunk`。

```bash
# 1) 前端（Rust→WASM，无 Node.js）
trunk build            # 产物在 dist/

# 2) 桌面壳（debug 构建使用 devUrl，需先 trunk serve）
trunk serve &          # 端口 1420
cargo run -p traysupervisor

# 正式产物（直接读 dist/，不依赖 dev server）
trunk build && cargo build --release -p traysupervisor
```

测试：

```bash
cargo test -p traysupervisor-core              # 单元 + 集成（会 spawn 真实进程）
cargo check --workspace --all-targets          # 全量编译检查
```

运行后从系统托盘进入：菜单列出服务（状态圆点 + PID）→ 启动/停止/重启/查看日志/终端视图 → 设置…（增删改查、env KV 表、Matcher 表、校验错误逐项展示）。

配置文件：`~/.traysupervisor/config.ini`（Unix 权限 0600），手改后自动热加载。

## 目录结构

```
├── crates/core/          # 纯 Rust 核心（无 tauri 依赖，NFR-8）
│   └── src/
│       ├── config/       # INI 解析/校验/env 展开/热载（CONFIG-SPEC）
│       ├── service/      # 状态机、spawn/stop、平台分支（setsid / Job Object）
│       ├── log/          # 环形缓冲 + file-rotate 轮转
│       ├── term.rs       # vte 只读终端仿真（后端渲染，D-3）
│       ├── matcher.rs    # 日志正则 → 托盘快捷项
│       ├── events.rs     # 广播事件总线（托盘与前端共用）
│       └── watcher.rs    # 配置文件热加载
├── src-tauri/            # Tauri 壳：commands / tray（持久菜单 + 就地更新）/ 窗口管理 / 事件桥接 / 信号处理
├── src/                  # Leptos 前端：设置页 / 日志页 / 终端页（hash 路由）
├── Trunk.toml / index.html
└── docs/                 # SRS / HLD / CONFIG-SPEC / CLI-SPEC / SYSTEM-TEST / FAQ（常见问题清单）
```

## 已知限制

- **Windows 分支未在真机回归**：Job Object 树杀路径按 `spikes/windows/job-object-kill-tree`（已验证 PASS）时序实现，发布前需在 Windows 11 冒烟；PTY 模式经 ConPTY（portable-pty），Windows 下停止为即时终止（无优雅信号可用）。
- Linux 托盘依赖桌面环境（GNOME 需 AppIndicator 扩展，见 SRS R2），本 MVP 未实现启动检测提示（NFR-7 的降级提示列入首个正式版）。
- 日志输出目标支持「仅内存 / 内存+文件」（CONFIG-SPEC 文件格式不表达「仅文件」）。
- Matcher 无隐式内置规则：新建服务时默认预填内置 HTTP URL 规则（FR-5.3），删除后空配置即无快捷项；非 URL 命中项在菜单中显示为禁用。
- `traysvctl` CLI（P1）、系统通知（P1）、开机自启（P2）、交互式终端（二期）未实现。

## 许可证

本项目采用 [PolyForm Noncommercial 1.0.0](./LICENSE) 协议：**任何非商业用途**（个人使用、学习、研究、修改、再分发）免费；**商业使用**（销售、付费服务、商业产品集成等）需获得版权人（find-partner）的单独商业授权。

> 注意：按 OSI 定义这不是"开源"协议，而是"源码可得"（source-available）——源码公开可读，但保留了商业权利。这正是本项目的意图：人人可免费用，只有版权人可以商用。

## 发布

`git tag vX.Y.Z && git push origin vX.Y.Z` 触发三平台自动构建（macOS universal dmg / Windows msi+安装器 / Linux deb+AppImage），产物自动附到 GitHub Releases。支持矩阵：Windows 10+、macOS 10.13+、Linux（WebKitGTK 4.1）。
