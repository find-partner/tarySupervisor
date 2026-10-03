# 命令行工具规格（CLI-SPEC）

| 项目 | 内容 |
|------|------|
| 工具名 | `traysvctl`（暂定名） |
| 版本 | v0.2（草案，与 SRS/HLD 同步） |
| 日期 | 2026-10-03 |
| 依据 | [SRS](./SRS.md) FR-9（P1）、[HLD](./HLD.md) §3.7 / §4、[CONFIG-SPEC](./CONFIG-SPEC.md) |
| 状态 | 概要设计定稿前配套规格 |

---

## 1. 定位

- **supervisorctl 风格**的管理命令行：为脚本、CI、远程 SSH 会话提供对常驻 TraySupervisor 应用的查询与控制入口。
- **不做守护进程**（SRS C3 / HLD D-2 不变）：`traysvctl` 只是常驻托盘应用的客户端，服务托管与状态始终位于托盘应用进程内。
- 与 GUI **共用 `crates/core`**（配置解析/校验、状态机语义、退出码语义），保证命令行、托盘菜单、设置窗口三侧行为与文案一致。

## 2. 体系结构与本地 IPC

```
traysvctl（crates/cli，clap 解析参数）
   │  JSON-RPC 2.0，NDJSON 帧（每行一个请求/响应对象）
   ▼
本地 IPC 端点（常驻托盘应用监听，HLD §3.7 app::ipc）
   ├─ Unix:    ~/.traysupervisor/traysv.sock   （Unix domain socket，权限 0600）
   └─ Windows: \\.\pipe\traysupervisor-ctl     （Named Pipe，ACL 限当前用户）
   ▼
crates/core（service 管理器 / config 模块）
```

- 端点随常驻应用启动创建、退出时清理；仅本机可达，无任何网络监听。
- 安全：socket 0600 / pipe 当前用户 ACL（NFR-5 延伸）；方法白名单固定（§4），不提供任意命令执行；每个连接同时至多一个未完成请求，无服务端主动推送。
- 方法集与 HLD §4.1 Command 的对应：`list` → `list_services`；`start` / `stop` / `restart` → 同名 Command；`tail` → `get_log`（增量拉取）；`reread` / `update` → `reload_config(mode)`；另有 `shutdown`、`version`。
- `tail -f` 用**长连接逐行响应**实现（同一方法，服务端持续写出 `LogAppended` 增量），是协议中唯一的流式例外。

## 3. 全局选项

| 选项 | 默认 | 说明 |
|------|------|------|
| `--config <路径>` | `~/.traysupervisor/config.ini` | 本地配置文件路径（`validate` / `reread` 提示用） |
| `--socket <路径>` / `--pipe <名称>` | 平台默认值 | 覆盖 IPC 端点 |
| `--timeout <秒>` | 30 | 单请求超时（对 `tail -f` 不生效） |
| `--json` | 关 | 机器可读输出（P2） |
| `-h` / `--help`、`-V` / `--version` | — | 帮助 / 版本 |

## 4. 子命令

| 子命令 | 说明 |
|--------|------|
| `status [<id>...]` | 列出服务：`<id>　<状态>　<PID>　<运行时长>　<说明>`；无参数 = 全部。状态集合与 FR-1.5 一致（STOPPED/STARTING/RUNNING/STOPPING/BACKOFF/EXITED/FATAL） |
| `start <id>... \| all` | 启动；立即返回，最终状态经 `status` 查询；`all` = 全部非 RUNNING 服务 |
| `stop <id>... \| all` | 停止；服务端执行 FR-3.3 流程（优雅 → 超时强杀整树） |
| `restart <id>... \| all` | 重启 = 停止 → 等待完全退出 → 启动（FR-3.4） |
| `tail [-f] [-n <行数>] <id>` | 输出该服务合并日志末尾行（默认 20 行），每行带 `OUT`/`ERR` 流标识前缀；`-f` 持续跟随，Ctrl-C 退出 |
| `reread` | 重新读取配置文件并**报告**与当前配置的差异（added / changed / removed），不应用 |
| `update [<id>...]` | **应用** `reread` 所报差异：removed 优雅停止并移除；changed 重启受影响服务；added 加入菜单（不自动启动，见 CONFIG-SPEC §8）；无差异时输出"未检测到变更" |
| `shutdown` | 通知常驻应用执行退出流程（逐个停止运行中服务后退出，FR-1.8 / FR-3.7）；应用未运行时幂等成功 |
| `validate` | 离线校验配置文件（无需连接），错误逐项输出（CONFIG-SPEC §7） |
| `version` | 输出 `traysvctl` 版本；能连接时常驻应用版本一并输出 |
| `help [<命令>]` | 帮助 |

说明：`reread` / `update` 拆分为两步是对 supervisord 语义的保留；GUI 的文件热载（notify）自动完成 reread+update，两者互不冲突（同一单写者通道串行应用，HLD RD-4）。

## 5. 应用未运行时的行为

IPC 端点不存在或连接被拒即视为"应用未运行"：

| 子命令 | 行为 |
|--------|------|
| status / start / stop / restart / tail / reread / update | stderr 提示 `TraySupervisor 应用未运行（请先启动托盘应用）`，退出码 3 |
| shutdown | 提示 `应用未运行，无需关闭`，退出码 0（幂等） |
| version / validate / help | 纯本地执行，正常返回 |

## 6. 退出码

| 码 | 含义 |
|----|------|
| 0 | 成功（`status`：所列服务全部 RUNNING） |
| 1 | 命令失败：服务不存在、当前状态不允许该操作（如 start 运行中服务）、`status` 存在非 RUNNING 服务（对齐 supervisorctl） |
| 2 | 用法错误；`validate` 校验失败 |
| 3 | 常驻应用未运行（IPC 连接失败） |
| 4 | 请求超时或 IPC 通信错误 |

## 7. 输出示例

```
$ traysvctl status
dev-docs     RUNNING    4213   0:12:31   pid 4213, uptime 0:12:31
api-server   FATAL      -      -         启动重试耗尽（startretries=5）

$ traysvctl reread
dev-docs: changed
api-server: removed
worker-1: added

$ traysvctl update
dev-docs: 已重启以应用配置
api-server: 已停止并移除
worker-1: 已加入菜单（autostart=false，未自动启动）

$ traysvctl tail -n 2 dev-docs
[OUT] Serving at http://127.0.0.1:8000
[OUT] 2026-10-03 09:12:01 GET / 200
```

## 8. 交付形态与打包

- workspace 新增 `crates/cli`（bin 名 `traysvctl`），依赖：`clap`、`crates/core`（类型与配置校验）、IPC 客户端（tokio + UDS/named pipe）；**不依赖 tauri**，构建产物为单一静态可执行文件。
- 随各平台安装包附带（NSIS 额外文件 / dmg / deb 与 AppImage 内含），另提供三平台独立压缩包下载。
- Windows 产物随应用一并代码签名（SRS NFR-10：SAC 强制模式会拦截未签名 exe）。
