# 系统测试计划与记录（SYSTEM-TEST）

| 项目 | 内容 |
|------|------|
| 版本 | v0.3（与 SRS v0.5 同步） |
| 日期 | 2026-10-06 |
| 依据 | [SRS](./SRS.md) §7 验收与交付、[HLD](./HLD.md) §8 测试策略 |
| 范围 | MVP（P0）：FR-1 ~ FR-6 及相关 NFR |
| 状态 | 自动化部分（A1~A4）已在本机通过；三平台手动矩阵（ST）待执行 |

---

## 1. 测试层级

| 层级 | 内容 | 执行方式 | 位置 |
|------|------|----------|------|
| L1 单元 | 配置解析/校验/env 展开/大小单位/轮转/Matcher/vte 序列/命令行切分/粘贴解析/平台解析链与合并 | `cargo test -p traysupervisor-core --lib` | `crates/core/src/**`（104 个） |
| L2 集成 | 真实进程 spawn/停止/自动重启/PTY/热加载（Unix） | `cargo test -p traysupervisor-core` | `crates/core/tests/service_tests.rs`（11 个） |
| L3 系统·脚本化 | 托盘应用整体行为（进程、文件、信号、菜单就地更新） | `scripts/smoke-unix.sh`（S-01~S-05）/ `scripts/smoke-windows.ps1`（S-06~S-08） | `scripts/` |
| L4 系统·手动 GUI | 托盘菜单交互、窗口渲染、对话框 | 人工按 §5 用例（ST-01~ST-22）执行 | 本文记录表 |
| CI 回归 | 三平台编译 + L1/L2 + 前端构建 | push/PR 自动触发 | `.github/workflows/ci.yml` |

> GUI 自动化限制：Tauri 的 WebDriver（tauri-driver）仅支持 Windows（WebView2）与 Linux（WebKitGTK）；
> **macOS 的 WKWebView 不支持 WebDriver**，故 L4 在 macOS 上只能人工执行。二期可在 Win/Linux 引入 tauri-driver。

## 2. 自动化清单与覆盖映射

| 编号 | 执行 | 覆盖的 SRS 验收点 |
|------|------|-------------------|
| A1 | `cargo test -p traysupervisor-core --lib`（104 单元） | FR-2.4（校验/切分/展开）、FR-2.2（大小单位 parse/format）、FR-4.2/4.3（环形缓冲/轮转/截断）、FR-5.1/5.2/5.4/5.6（匹配/去重/LRU/URL 白名单）、FR-6.1（vte 序列）、§3.4 INI 语义、FR-2.12（粘贴解析）、FR-2.13（解析链/env 标签合并/往返/diff） |
| A2 | `cargo test -p traysupervisor-core`（含 11 集成，Unix） | FR-3.1~3.4（状态机/停止/重启/BACKOFF/FATAL）、FR-2.3（子进程 env）、FR-5.2/5.3/5.5（URL 预设规则提取/重启清空）、FR-6.2（PTY 捕获+快照）、FR-2.6（热载 diff） |
| A3 | `scripts/smoke-unix.sh`（macOS/Linux，8 项断言） | S-01：FR-2.1 autostart + FR-3.7 信号退出清理；S-02：FR-4.3 轮转（100KB×3 缩减版，顺带覆盖 maxbytes 单位解析）；S-03：FR-2.6 热加载 applied/沿用；S-04：NFR-5 权限 0600；S-05：FR-1.6 就地更新（状态扰动期零重建） |
| A4 | `scripts/smoke-windows.ps1`（Windows） | S-06：FR-3 树杀（cmd→ping 孙进程）+ KILL_ON_JOB_CLOSE 硬杀兜底；S-07：FR-2.6 热加载；S-08：配置文件存在性 |
| A5 | CI 三平台矩阵 | Windows runner 编译 Job Object / windows-sys 平台分支（当前最大盲区的最小回归）；Linux/macOS 全量 L1/L2；trunk WASM 构建 |

就地更新测量口径（S-05，HLD D-7）：5 个 `sleep 1` + `autorestart=always` 服务运行 6 秒，产生 ≈50+ 状态事件；
`RUST_LOG=debug` 断言「托盘菜单项已就地更新」≥20 且「托盘菜单已重建」= 0（仅服务集合变化才重建）。
菜单展开中不消失（FR-1.6 的用户可见口径）由 ST-02 人工验证。

## 3. 测试环境矩阵

| 平台 | 版本 | 前置 | 说明 |
|------|------|------|------|
| Windows | 11 x64 | MSVC 工具链；WebView2 Runtime | Smart App Control 可能拦截未签名 exe（SRS NFR-10 / spikes README） |
| macOS | 14（Apple Silicon / Intel 均可） | Rust + trunk | 无额外依赖 |
| Linux | Ubuntu 24.04 | GNOME（装 AppIndicator 扩展）与 KDE 各测一轮 | WebKitGTK 4.1 系统库 |

构建：`cargo build -p traysupervisor`（debug 即可）；如需 GUI 窗口渲染则 `trunk build` 后用 release 或先起 `trunk serve`。

### 3.1 最低系统版本与发布产物（release.yml）

| 平台 | 最低版本 | 发布产物 | 依据 |
|------|---------|---------|------|
| Windows | **10 x64**（Win7/8 不支持） | msi + nsis 安装器 | Rust ≥1.78 的 Windows 目标最低 Win10；WebView2 运行时对 Win7/8 的支持已于 2023-01 终止（最后版本 109 固定版，无安全更新）；Tauri v2 / windows-sys 0.59 无 Win7 官方支持。如确需 Win7 须降级 Tauri v1 + Rust ≤1.77 工具链，属产品级决策，不做承诺 |
| macOS | **10.13+**（12 完全覆盖） | universal dmg（Intel + Apple Silicon） | release 构建设置 `MACOSX_DEPLOYMENT_TARGET=10.13`（Tauri v2 最低要求）；Apple Silicon 最低 11。发布产物在 macos-14 runner 构建，低版本系统按部署目标兼容 |
| Linux | glibc 较新发行版（Ubuntu 24.04 基线） | deb + AppImage | WebKitGTK 4.1 运行时 |

产物由 `git tag vX.Y.Z && git push origin vX.Y.Z` 触发 release.yml 自动构建并附到 GitHub Release；未打 tag 前不会出现任何 Release 结果（此前仓库无 tag 亦无发布 job，故 Release 页为空）。

## 4. 脚本化用例（L3）

权威步骤以脚本内容为准（断言内嵌、自动备份/恢复用户配置）：

- **S-01 / S-06~07 进程与清理**：autostart 拉起并校验父子关系 → 结束应用 → 托管进程零残留；
  Windows 额外验证 cmd→ping 孙进程整树终止与硬杀兜底（`Stop-Process -Force` 后 Job 关闭清理）。
- **S-02 日志轮转**：`seq 1 200000`（≈1.3MB 输出）→ 100KB 上限 × 保留 3 份 → 断言文件数 2~4、总占用 ≤500KB。
  SRS 验收原文（10MB×3 灌 50MB）为正式版用例，见 ST-09。
- **S-03 / S-07 热加载**：追加合法节 → 日志出现「配置热加载已应用」；写入未知键 → 出现「配置热加载校验失败」（沿用上次配置）。
- **S-04 权限**：删除配置后由应用创建，断言 0600。
- **S-05 防抖**：见 §2 测量口径。

## 5. 手动 GUI 用例（L4）

> 执行前：`trunk build && cargo run -p traysupervisor`（debug 需先 `trunk serve`；或直接 release）。
> 每项完成后在 §6 记录表勾选对应平台。

### FR-1 托盘与主菜单

- **ST-01 菜单结构与状态色（FR-1.1/1.2/1.3/1.4/1.5）**
  前置：配置 3 个不同状态的服务（一个 RUNNING、一个 autostart=false 停止、一个命令错误 FATAL）。
  步骤：① 单击托盘图标 ② 逐项展开服务子菜单 ③ 观察 tooltip。
  预期：主菜单 = 服务列表→分隔线→设置…→退出；子菜单含状态行（`● RUNNING (PID n)` 等七态文案）、启动/停止/重启按状态启用禁用、查看日志、终端视图；状态色符合 FR-1.5；tooltip =「运行中 n / 共 m 个服务」；Windows 右键同样弹出。
- **ST-02 状态刷新与菜单稳定（FR-1.6 / NFR-3）**
  步骤：`RUST_LOG=debug` 启动；① 对 RUNNING 服务执行停止/启动各一次，秒表观察菜单变化 ② 菜单展开期间令一服务状态变化（如 kill 一个 RUNNING 服务）③ 再执行 §2 S-05 场景。
  预期：状态变更到菜单可见 ≤1s；菜单展开期间状态变化**菜单不消失**（项内容就地更新，最迟下次展开生效）；S-05 就地更新 ≥20 且零重建（自动化已覆盖）。
- **ST-03 退出确认（FR-1.8）**
  步骤：有 ≥1 个 RUNNING 服务时点「退出」；再在全部停止时点一次。
  预期：运行中 → 对话框列出将被停止的服务，确认后逐个停止再退出（进程检查无残留）；空闲 → 直接退出。
- **ST-04 单实例（FR-1.9）**
  步骤：应用运行中再次启动同一二进制。
  预期：不出现第二个托盘图标，已有实例被唤醒/聚焦（设置窗口）。
- **ST-05 Matcher 快捷项（FR-5.2/5.3 验收）**
  前置：服务 `command=python3 -u -m http.server 8000`（无需配置 Matcher）。
  步骤：启动服务，观察菜单 ≤1s；点击「🌐 打开 http://…」。
  预期：快捷项出现（≤1s），点击调起系统默认浏览器打开该 URL。
- **ST-06 Matcher 重启清空（FR-5.5）**
  步骤：对 ST-05 服务执行重启，观察菜单快捷项。
  预期：旧项随重启消失；新一次输出重新命中后恢复。

### FR-2 配置管理

- **ST-07 env KV 表与替换（FR-2.1/2.3 验收）**
  步骤：设置窗口新建服务，env 表逐行添加 3 个变量，其中 1 行开启替换引用另一行（如 `A=x`、`B=!${A}/bin`）；点「预览展开结果」；保存并启动；日志窗口查看（服务用 `env` 程序可打印全部环境变量）。
  预期：预览显示 `B=x/bin`；子进程环境中变量正确。
- **ST-08 校验拦截（FR-2.4）**
  步骤：① env 值填 `${UNDEFINED_VAR}` 开启替换后保存 ② Matcher 填入 `(unclosed` 保存 ③ id 填大写/空格保存。
  预期：三种情况均被拦截，错误逐项显示且未写入配置文件。
- **ST-09 删除运行中服务（FR-2.5）**
  步骤：对 RUNNING 服务点删除。
  预期：出现确认对话框（提示将先停止）；确认后服务停止并从菜单消失，进程无残留。

### FR-3 生命周期

- **ST-10 Windows 停止无残留（FR-3.3 验收）**
  平台：Windows。前置：`command=python -m http.server 8000`。
  步骤：启动 → 菜单点停止；用任务管理器/`tasklist` 检查 python 进程。
  预期：无残留（机制层由 S-06 的 cmd→ping 孙进程场景自动化覆盖）。
- **ST-11 Unix 外部 kill 后状态恢复（FR-3.3 验收）**
  平台：macOS/Linux。前置：RUNNING 服务，`autorestart=no`。
  步骤：`pkill -TERM -f <服务命令>`；观察托盘菜单 10 秒。
  预期：状态变为 EXITED（FR-3.1：外部意外终止 = EXITED；SRS 验收原文写 STOPPED，按状态机语义以 EXITED 执行——见 §7 偏差记录）；经菜单「启动」可复活。
- **ST-12 kill -9 自动拉起（FR-3.1/3.2 验收）**
  前置：`autorestart=always, startsecs=0` 服务。
  步骤：`kill -9 <PID>`；观察菜单（BACKOFF 橙点 → RUNNING 绿点，PID 变化）。
  预期：自动重启（集成测试 A2 已覆盖语义层，此处观察 UI 呈现与退避显示）。

### FR-4 日志

- **ST-13 日志窗口交互（FR-4.4/4.5/4.6）**
  步骤：打开 RUNNING 服务日志窗口：① 上滚（自动滚动暂停）→ 回到底部（恢复）② 复制全部 ③ 清空视图 ④ 关闭窗口后持续产生输出（如 `yes` 类服务）→ 重新打开。
  预期：滚动行为符合描述；关闭窗口不影响捕获，重开后历史在；ERR 流红色；时间戳 + [OUT]/[ERR] 前缀。
- **ST-14 正式轮转验收（FR-4 验收原文）**
  前置：`stdout_logfile` 配置 maxbytes=10MB、backups=3（GUI 日志页单文件上限同样填 `10MB`）。
  预期：目录中存在主文件 + ≤3 个分块，总占用 ≈ ≤42MB + 当前写入余量；窗口滚动流畅。

### FR-6 终端视图

- **ST-15 PTY 彩色进度条（FR-6.1/6.2 验收）**
  前置：`spawn_mode=pty`，命令：
  `python3 -u -c "import time,sys;[(sys.stdout.write('\r\033[32m'+'█'*(i%40)+'\033[0m %d'%i),sys.stdout.flush(),time.sleep(0.1)) for i in range(80)]"`
  步骤：打开终端视图观察；关闭窗口再重开；随后重启服务再观察。
  预期：绿色进度条**逐行正确换行**原地刷新（无残影串行、不连成一行）；颜色渲染正确；关窗不影响服务，重开后当前会话画面保留；**重启后终端屏清空**（上一会话输出不保留，FR-6.6），新会话输出从空白屏开始。
- **ST-16 管道模式退化（FR-6.3）**
  步骤：对 `spawn_mode=pipe` 服务打开「终端视图（纯文本）」。
  预期：呈现纯文本日志流（黑底），底部状态栏提示只读。

### 全局

- **ST-17 空闲资源（NFR-1）**
  步骤：全部服务停止、无窗口打开，`ps -o rss` / 任务管理器记录 RSS。
  预期：≤ 80MB（记录实测值）。
- **ST-18 日志窗口工具栏（FR-4.4）**
  「打开日志目录」在未配置 `stdout_logfile` 时给出明确提示；配置后打开正确目录。
- **ST-19 GNOME 降级提示（NFR-7 / SRS R2）**
  平台：Ubuntu GNOME（无 AppIndicator 扩展）。
  状态：**本 MVP 未实现检测提示，列为豁免项**（首正式版交付）；手动验证托盘图标不可见时应用不崩溃即可。
- **ST-20 编辑页服务操作栏（FR-2.11 验收）**
  步骤：设置页选中一个服务：① 观察标签栏上方操作栏的状态显示与按钮可用性 ② 点「启动/停止/重启」各一次 ③ 点「查看日志」「终端视图」。
  预期：状态（色点 + 文本 + PID）与托盘菜单一致且随 `service-state-changed` 实时刷新；启停按钮按状态启用/禁用；非法操作（如重复启动）在栏内提示；日志/终端窗口正确打开（或聚焦已开窗口）。
- **ST-21 三处状态一致（FR-1.10 验收）**
  步骤：同时可见托盘菜单、设置页服务列表、编辑页操作栏；经任一处（或 CLI 外部 kill）改变服务状态。
  预期：三处状态在 1s 内一致更新（同一数据源 + 同一事件流，HLD D-15）。
- **ST-22 日志大小单位（FR-2.2 / D-16 验收）**
  步骤：① 日志页单文件上限填 `10MB` 保存，检查 config.ini ② 手写 `stdout_logfile_maxbytes=512KB` 热加载 ③ 填入 `abc` 或 `10TB` 保存。
  预期：① INI 回写 `stdout_logfile_maxbytes=10MB` ② 合法加载生效 ③ 保存被拦截并逐项报错。
- **ST-23 命令行粘贴解析（FR-2.12 验收）**
  步骤：① 新建服务第①步，向命令行框粘贴 `ENABLE_CORS=true USE_PROXY=true PROXY_URL=http://127.0.0.1:7890 open-websearch --port=1123` ② 在编辑页粘贴无前缀变量的普通命令 ③ 粘贴仅 `FOO=bar`（无程序）④ 手改命令行后点「解析」。
  预期：① 自动拆分：环境变量表出现 3 行（全部平台）、命令行变为 `open-websearch --port=1123`，并提示「已识别 3 个前缀环境变量」② 原样插入不拆分 ③ 提示「缺少命令」且输入不变 ④ 重新解析成功。
- **ST-24 平台变体与跨平台同步（FR-2.13 验收）**
  步骤：① 编辑页「基础配置 → 平台变体」为 Windows 子页填命令行 `C:\tools\app.exe --port=1123`、工作目录、捕获模式保存，检查 config.ini ② 平台环境变量表为某行选「windows」并填平台路径，保存后检查 INI `@windows` 标签 ③ 修改 Windows 变体保存，观察本机运行中服务的重启提示 ④ 修改「默认命令行」保存。
  预期：① INI 出现 `command.windows=…` 等键且编辑回显完整 ② INI 出现 `KEY="…"@windows` ③ 非 Windows 平台无重启提示（生效值不变）④ 出现「需重启生效」提示；「本机」角标与解析链提示正确。
- **ST-25 当前平台声明（FR-2.13 验收）**
  步骤：① 全局节手写 `platforms=home-mac:macos, build-box` 与 `current_platform=home-mac`，保存触发热载 ② 设置页观察解析链提示与平台下拉选项 ③ 为 `home-mac` 配命令行变体后启动服务，对比 `ps` 实际进程 ④ 手写 `current_platform=ghost`。
  预期：① 热载成功 ② 平台列/变体子标签出现自定义平台且 home-mac 带「本机」角标 ③ 实际拉起 home-mac 变体命令 ④ 校验失败横幅提示 current_platform 未注册。

## 6. 结果记录矩阵

| 用例 | Windows 11 | macOS 14 | Ubuntu 24.04 (GNOME) | Ubuntu 24.04 (KDE) |
|------|:---:|:---:|:---:|:---:|
| ST-01 菜单结构与状态色 | ☐ | ☐ | ☐ | ☐ |
| ST-02 状态刷新 ≤1s | ☐ | ☐ | ☐ | ☐ |
| ST-03 退出确认 | ☐ | ☐ | ☐ | ☐ |
| ST-04 单实例 | ☐ | ☐ | ☐ | ☐ |
| ST-05 Matcher 快捷项 ≤1s | ☐ | ☐ | ☐ | ☐ |
| ST-06 重启清空快捷项 | ☐ | ☐ | ☐ | ☐ |
| ST-07 env KV 表与替换 | ☐ | ☐ | ☐ | ☐ |
| ST-08 校验拦截 | ☐ | ☐ | ☐ | ☐ |
| ST-09 删除运行中服务 | ☐ | ☐ | ☐ | ☐ |
| ST-10 Windows 停止无残留 | ☐ | —（A2/A3 等效） | —（A2/A3 等效） | —（A2/A3 等效） |
| ST-11 Unix kill 后状态 | — | ☐ | ☐ | ☐ |
| ST-12 kill -9 自动拉起 | ☐ | ☐ | ☐ | ☐ |
| ST-13 日志窗口交互 | ☐ | ☐ | ☐ | ☐ |
| ST-14 正式轮转 50MB | ☐ | ☐ | ☐ | ☐ |
| ST-15 PTY 彩色进度条 | ☐ | ☐ | ☐ | ☐ |
| ST-16 管道模式退化 | ☐ | ☐ | ☐ | ☐ |
| ST-17 空闲内存 ≤80MB | ☐ | ☐ | ☐ | ☐ |
| ST-18 打开日志目录 | ☐ | ☐ | ☐ | ☐ |
| ST-19 GNOME 降级提示 | — | — | 豁免（MVP） | — |
| ST-20 编辑页操作栏 | ☐ | ☐ | ☐ | ☐ |
| ST-21 三处状态一致 | ☐ | ☐ | ☐ | ☐ |
| ST-22 日志大小单位 | ☐ | ☐ | ☐ | ☐ |
| ST-23 粘贴解析 | ☐ | ☐ | ☐ | ☐ |
| ST-24 平台变体与同步 | ☐ | ☐ | ☐ | ☐ |
| ST-25 当前平台声明 | ☐ | ☐ | ☐ | ☐ |

脚本化用例记录：S-01~S-05（macOS 本机 2026-10-03 全过，8/8 断言）；S-06~S-08 待 Windows 真机。
CI：A5 待仓库接入 GitHub Actions 后首次运行确认。

## 7. 与 SRS 的偏差 / 豁免记录

| 项 | 说明 |
|----|------|
| ST-11「回到 STOPPED」 | SRS FR-3 验收原文为 STOPPED；实现按 FR-3.1 状态机语义，外部意外终止进入 EXITED（经菜单/CLI 启动后回 RUNNING）。验收以 EXITED 执行。 |
| ST-19 GNOME 降级提示 | NFR-7 的启动检测+提示属首个正式版；MVP 豁免，仅验证不崩溃。 |
| FR-4 输出目标 | 「仅文件」模式无 INI 表达（CONFIG-SPEC 以 stdout_logfile 是否为空表达），GUI 提供「仅内存 / 内存+文件」两档。 |
| Windows PTY 停止 | ConPTY 下无优雅信号可用，停止为即时终止（README 已声明）；树杀语义由 Job/ConPTY 会话保证。 |
| macOS GUI 自动化 | WKWebView 无 WebDriver 支持，L4 全部人工执行（§1 说明）。 |
