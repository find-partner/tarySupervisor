# 配置文件规范（CONFIG-SPEC）

| 项目 | 内容 |
|------|------|
| 版本 | v0.5（与 SRS/HLD v0.5 同步） |
| 日期 | 2026-10-06 |
| 依据 | [SRS](./SRS.md) FR-2 / FR-3 / FR-4 / FR-5，[HLD](./HLD.md) §3.1 |
| 状态 | 概要设计定稿前配套规范 |

---

## 1. 目标与格式

- 配置文件采用**与 supervisord 相似的 INI 风格**：`[program:<id>]` 分节、`key=value` 键值对、`#` / `;` 注释、UTF-8 编码。
- 目标：复用 Python Supervisor 用户的心智与习惯——字段名与语义尽量对齐 supervisord；本项目特有功能按同风格扩展命名。
- 单文件路径：`~/.traysupervisor/config.ini`（Windows 为 `%USERPROFILE%\.traysupervisor\config.ini`）；Unix 权限收紧为 0600（SRS NFR-5）。
- **数据目录解析优先级**（`crates/core/src/lib.rs` `config_dir()`，按序取第一个命中）：
  1. 环境变量 `TRAYSUPERVISOR_DATA_DIR`——PortableApps.com 便携包由启动器注入 `%PAL:DataDir%`，数据整体落在 `Data\`（config.ini / logs\ / EBWebView\）；
  2. exe 同级存在 `Data` 目录——免启动器的绿色解压包布局，数据取该 `Data` 目录；
  3. 回落 `~/.traysupervisor`——安装版与 macOS / Linux 常规分发形态，行为与历史版本一致。

  命中 1/2 即为**便携形态**（`is_portable()`）：除数据目录外，WebView2 用户数据目录也收进 `<数据目录>\EBWebView`（Windows 留痕规避）；服务配置中的 `~/...` 仍展开为宿主 home（服务本就运行在宿主机上，属预期）。
- 解析：`rust-ini` 读入 → 映射到 HLD §3.1 的语义模型（`ServiceConfig` 等结构体不变）→ 映射层完成全部校验（FR-2.4）。**未知键视为校验错误**（尽早暴露拼写错误）。

## 2. 全局节 `[traysupervisor]`

对应 supervisord 的 `[supervisord]` 节；可选，省略时全部取默认值。

| 键 | 类型 | 默认 | 说明 | 对应需求 |
|----|------|------|------|---------|
| log_lines | int | 2000 | 每服务内存环形缓冲行数（100–100000） | FR-4.2 |
| notify_on_failure | bool | true | 服务异常退出（EXITED/FATAL）时系统通知（P1） | FR-7.1 |
| platforms | 逗号列表 | 空 | **自定义平台注册表**（FR-2.13）：每项 `名字[:家族]`，名字为 slug（`[a-z0-9-_]`，不得与内置平台同名），家族须为内置平台名（`windows` / `macos` / `linux`），如 `platforms=home-mac:macos, office-win:windows`；内置三平台恒可用、无需声明 | FR-2.13 |
| current_platform | string | 空 | **当前所在平台**（FR-2.13）：取值须为内置或已声明平台；空 = 按内置自动检测（`std::env::consts::OS`）。自定义平台无法自动检测，必须声明。注意：该键属机器本地状态，随配置文件同步到新机器后需改一次 | FR-2.13 |

**平台解析链**（服务级变体取用规则，按序取第一个命中、均未命中回退默认值）：
`current_platform` → 其 OS 家族（自定义平台且声明了家族时）→ 自动检测的内置 OS（链中尚未含家族时附加）。

## 3. 服务节 `[program:<id>]`

- `<id>` 即服务 id：仅 `[a-z0-9-_]`，全文件唯一；用于菜单项 ID 编码（FR-1.7）与 CLI。
- 各节在文件中的先后顺序 = 菜单顺序与启动顺序。
- 每节对应一个服务；模型层 `id` 字段直接取自节名。

### 3.1 与 supervisord 对齐的字段

| 键 | 类型 | 默认 | 说明 | supervisord 对照 | 对应需求 |
|----|------|------|------|------------------|---------|
| command | string | 必填 | **完整命令行（程序 + 参数）单行**，按 §3.4 的 shlex 风格规则切分为"程序 + 参数数组"，**不经 shell**（FR-3.6；例外：`shell=true`，见 §3.5） | `command` | FR-2.1 |
| directory | 路径 | 用户目录 | 工作目录，支持 `~` 前缀展开 | `directory` | FR-2.1 |
| autostart | bool | false | 应用启动时自动启动 | `autostart` | FR-2.1 |
| autorestart | enum | no | `no` / `on-failure` / `always`；on-failure = 退出码 ≠ 0 或被信号/强杀终止 | 语义同 `false` / `unexpected` / `true`，枚举值采用更直读的别名 | FR-2.1 |
| startsecs | int（秒） | 3 | 进程存活超过该秒数（或产生首行输出）判定 STARTING → RUNNING | `startsecs`（supervisord 默认 1，本项目取 3 降低闪退误判） | FR-3.1 |
| startretries | int | 5 | BACKOFF 最大重试次数，耗尽进入 FATAL | `startretries`（supervisord 默认 3） | FR-3.1 |
| stopwaitsecs | int（秒） | 10 | 优雅停止等待时长，超时强杀整树（模型字段 `stop_timeout_secs`） | `stopwaitsecs` | FR-2.1 / FR-3.3 |
| environment | 有序 KV 列表 | 空 | `KEY="value"` 逗号分隔多对，**书写顺序即注入顺序**（条目间可引用），见 §4 | `environment` | FR-2.3 |
| redirect_stderr | bool | false | true：stderr 并入 stdout 单流；false（默认）：两流分别捕获并以流标识区分 | `redirect_stderr`（supervisord 默认 true，本项目默认保留流标识） | FR-4.1 |
| stdout_logfile | 路径 | 空 | 日志文件：stdout/stderr 写同一文件，行内带时间戳与流标识；空 = 仅内存。支持绝对路径或 `~` 前缀；相对路径相对配置文件所在目录 | `stdout_logfile` | FR-2.2 |
| stdout_logfile_maxbytes | 大小字符串 | 10MB | 单文件上限，`0` = 不轮转；**推荐带单位**：`KB`/`MB`/`GB`（大小写不敏感，可省略 `B`，如 `512kb`、`10MB`、`1G`），兼容纯字节数（`10485760`）；保存回写为可读单位 | `stdout_logfile_maxbytes`（supervisord 同样支持数值后缀） | FR-2.2 |
| stdout_logfile_backups | int | 3 | 轮转分块保留份数（`.1 .2 …` 后缀） | `stdout_logfile_backups` | FR-2.2 |

### 3.2 项目扩展字段（supervisord 无）

| 键 | 类型 | 默认 | 说明 | 对应需求 |
|----|------|------|------|---------|
| name | string | = id | 菜单与 CLI 中的显示名 | FR-2.1 |
| spawn_mode | enum | pipe | `pipe`（管道捕获）/ `pty`（伪终端捕获，启用只读终端视图 FR-6） | FR-2.1 / FR-6 |
| term_interactive | bool | false | 终端视图**可写（交互）**：键盘/粘贴输入转发至服务 stdin；仅 `spawn_mode=pty` 有效，否则校验错误 | FR-6.4 |
| shell | bool | false | true：经用户登录 shell（`$SHELL -ilc`）启动，继承其配置环境（PATH、nvm 等）；见 §3.5 | FR-2.1 |
| logfile_compress | bool | false | 轮转出的旧分块 gzip 压缩 | FR-2.2 |
| matcher.N.pattern | regex | — | 第 N 条 Matcher 正则（Rust regex 语法），N 从 1 连续递增，顺序即匹配顺序 | FR-5.1 |
| matcher.N.template | string | `🌐 打开 {0}` | 快捷项文案，`{0} {1}` 引用捕获组 | FR-5.1 |
| matcher.N.max_items | int | 5 | 该条快捷项上限（≤ 10，LRU 淘汰） | FR-5.4 |
| command.&lt;平台&gt; | string | 空 | **平台命令行变体**（FR-2.13）：完整命令行单行，切分规则同 §3.4；`<平台>` ∈ 平台注册表（§2）；未命中的平台用默认 `command` | FR-2.13 |
| directory.&lt;平台&gt; | 路径 | 空 | 平台工作目录变体，支持 `~` 展开 | FR-2.13 |
| spawn_mode.&lt;平台&gt; | enum | 空 | 平台捕获模式变体（`pipe` / `pty`） | FR-2.13 |
| term_interactive.&lt;平台&gt; | bool | 空 | 平台交互终端变体；须与该平台 `spawn_mode` 组合合法（§7） | FR-2.13 |
| environment 行级标签 | `@平台` | 无 | 环境变量条目引号后可写 `@平台`（或 `@all`）：该行仅在该平台生效；无标签 = 全部平台；同名键平台条目**就地覆盖**全部平台条目（值与 expand 均以平台条目为准、位置不变），见 §4 | FR-2.13 |

### 3.3 暂不支持 / 二期（有意省略的 supervisord 字段）

| supervisord 字段 | 处理 | 原因 |
|------------------|------|------|
| stopsignal / stopasgroup / killasgroup | 不支持 | Windows 无信号：统一"TERM/CTRL_BREAK 尽力尝试 → 超时 → 整树强杀"（HLD D-5）；Unix `setsid + killpg`、Windows Job Object 天然整组/整树，无需逐项配置 |
| user | 不支持 | C5 不提权 |
| priority / `[group:x]` | 二期 | SRS §8"服务分组与优先级启动" |
| `[include]` | 不支持 | MVP 单文件 |
| `%(ENV_x)s` 插值 | 不支持 | 变量替换统一走 FR-2.3 的 `${VAR}` 规则（§4） |
| exitcodes | 二期可选 | on-failure 已按"非零退出码或被信号终止"定义 |

### 3.4 command 切分规则（对 shlex 的 Windows 友好偏离）

1. 空白（空格/Tab）分隔各 token；
2. 双引号包裹的部分整体为一个 token，内部空白不切分；
3. `\` 仅在其后紧跟 `"`、`\` 或空白时作为转义符，**其余情况字面保留**——因此 Windows 路径 `C:\Users\me\app.exe` 无需引号即可使用（这是与 Python `shlex` POSIX 模式的刻意差异，见 §9）；
4. 切分结果第一个 token 为程序（可执行文件路径或 PATH 可解析名），其余为参数数组（FR-3.6：直接传给目标程序，不经 shell）。

### 3.5 shell 启动模式与 PATH 兜底

GUI 应用从 Finder/启动台（launchd）拉起时只继承精简 PATH（`/usr/bin:/bin:/usr/sbin:/sbin`），用户在 `.zshrc`/`.bashrc` 里追加的 Homebrew / nvm 等路径全部缺失，`#!/usr/bin/env node` 类解释器查找会以 `env: node: No such file or directory`（exit 127）失败。两项机制解决该问题：

**PATH 兜底注入（默认生效，无需配置）**：spawn 时若服务 `environment` 未显式配置 `PATH`，运行时以 `$SHELL -ilc 'printf %s "$PATH"'`（交互+登录，等价终端环境；结果进程内缓存，5s 超时）取回 PATH 注入子进程环境；探测失败时回退静态目录（`/opt/homebrew/bin` 等常见位置）拼接到继承 PATH 之前。服务显式配置 `environment=PATH=...` 时以配置为准；env「预览」展示与实际注入一致。Windows 桌面进程本就从注册表获得完整 PATH，不做注入。

**`shell=true` 启动模式（按服务显式开启）**：目标命令经用户登录 shell 启动（Unix：`$SHELL -ilc '<命令行>'`；Windows：`%COMSPEC% /d /s /c "<命令行>"`）。命令行按 token 以 POSIX 单引号规则（Unix）/ MSVCRT 规则（Windows）重组，**shell 解析出的 argv 与直接 exec 完全一致**——shell 的作用是继承其配置环境（PATH、nvm、别名外的变量导出等），**不支持**管道、`&&` 等 shell 语法（token 会被当作普通参数，需要时请改用包装脚本）。`shell=true` 属于 changed 判定字段（§8），修改后需重启生效。

**兼容性声明（DEV-CONVENTIONS §1）**：`shell` 为可选键、缺省 false——旧配置文件（无 `shell` 键）在当前版本解析不受影响；含 `shell=true` 的配置文件在旧版本应用上会因「未知键」被拒绝，不做兼容。

### 3.6 命令行粘贴解析（FR-2.12，仅 GUI 交互层）

设置页命令行输入框支持粘贴 Unix 惯例的整行命令：按 §3.4 切分 token 后，从头扫描形如 `KEY=VALUE`（`KEY` 为 `[A-Za-z_][A-Za-z0-9_]*`）的前缀 token 记为环境变量候选，第一个不符合该形态的 token 即程序，其后全部为参数。识别出的环境变量 `expand=false`、无平台标签；重复 KEY 保留最后值、位置取首次出现；整行全为 `KEY=VALUE`（无程序）→ 报错「缺少命令」。粘贴无前缀环境变量的文本时原样插入（行为与普通粘贴一致）；「解析」按钮可对手动编辑后的内容重新解析。解析结果走既有校验/保存/落盘链路，**不产生新文件格式**。

## 4. environment 与变量替换

- 语法：逗号分隔多对 `KEY="value"`；value 必须双引号包裹，内部 `"` 写作 `\"`，内部逗号无需转义；键名大小写敏感且必须合法（不含 `=`、空白）。
- 值**默认字面量直传**（不解析任何语法，FR-2.3 expand=off）；**前缀 `!` 开启替换**：`KEY="!${BASE}/bin"`。
- **行级平台标签**（FR-2.13 / DESIGN §4.3）：条目引号后可选 `@平台`（`@all` 或无标签 = 全部平台）。生效表 = 标签命中当前平台解析链的条目（保持书写顺序）：同名键平台条目**就地覆盖**全部平台条目（后续条目的 `${VAR}` 引用取到覆盖后的值，与最终注入一致）；标签未命中的条目整体排除。同一 `(键, 标签)` 组合不得重复。
- 替换规则（FR-2.3 / HLD D-4）：`${VAR}` 先查本服务 environment 表中**排在其前面**的条目，再查系统环境；仅支持 `${VAR}` 一种语法；未定义 → 校验错误并阻止保存/启动；不支持转义、嵌套与递归展开。
- 示例（PATH_EXTRA 开启替换，引用其前面的 APP_HOME）：

```ini
environment=APP_HOME="C:\opt\myapp", PATH_EXTRA="!${APP_HOME}\bin", GREETING="hello, world"
```

## 5. Matcher

Matcher 是本项目的特有功能（日志正则 → 托盘快捷项），以编号键组表达有序列表：

```ini
matcher.1.pattern=https?://\S+
matcher.1.template=🌐 打开 {0}
matcher.1.max_items=5
matcher.2.pattern=Listening on port (\d+)
matcher.2.template=端口 {0}
```

- N 必须**从 1 开始连续递增**，顺序即匹配顺序；跳号视为校验错误。
- 节内无任何 `matcher.N.*` 键即视为**无规则**：运行期不会隐式启用任何内置规则（FR-5.3 的内置 URL 规则仅由新建服务向导预填落盘）。
- 每服务 Matcher 条数上限 20；`max_items` ≤ 10（FR-5.4）。
- 正则在保存/加载时编译校验（FR-2.4 / FR-5.5）。

## 6. 完整示例

```ini
# ~/.traysupervisor/config.ini
# TraySupervisor 配置（supervisord 风格）；完整规范见 docs/CONFIG-SPEC.md

[traysupervisor]
log_lines=2000
notify_on_failure=true
platforms=home-mac:macos, office-win:windows
current_platform=home-mac

[program:dev-docs]
name=文档站
command=python -m http.server 8000 --directory "C:\work\docs"
directory=C:\work\docs
autostart=true
autorestart=on-failure
startsecs=3
stopwaitsecs=10
stdout_logfile=~/.traysupervisor/logs/dev-docs.log
stdout_logfile_maxbytes=10MB
stdout_logfile_backups=3
logfile_compress=true
matcher.1.pattern=https?://\S+
matcher.1.template=🌐 打开 {0}
matcher.1.max_items=5

[program:api-server]
command=node --max-old-space-size=512 main.js
directory=~/work/api
spawn_mode=pipe
autostart=false
autorestart=always
environment=HOME="/home/dev", NODE_ENV="development", LOG_DIR="!${HOME}/logs/api", PORT="3000", LOG_DIR="!${LOCALAPPDATA}\logs"@windows
command.macos=/opt/homebrew/bin/node --max-old-space-size=512 main.js
directory.windows=C:\work\api
spawn_mode.windows=pipe
term_interactive.windows=false
redirect_stderr=false
```

## 7. 校验规则（FR-2.4）

加载与保存均执行，错误逐项输出（含节名与键名定位），任一错误即整体拒绝：

1. 节名：仅允许 `[traysupervisor]` 与 `[program:<id>]`；id 合法（slug）且全文件唯一；
2. `command` 非空，且可按 §3.4 规则切分出程序名；
3. 数值范围：`startsecs ≥ 0`、`stopwaitsecs ≥ 1`、`startretries 0–10`、`stdout_logfile_backups 0–100`、`stdout_logfile_maxbytes 0–1GB`（支持 `KB/MB/GB` 单位后缀或纯字节数；非法单位、非数字或溢出报错）、`log_lines 100–100000`、`max_items ≤ 10`、Matcher 条数 ≤ 20；
4. Matcher 正则可编译；`matcher.N` 编号连续；
5. environment 值必须 UTF-8；开启替换的值中 `${VAR}` 引用可解析（本表顺序 + 系统环境）且无环；
6. 枚举值合法（`autorestart`、`spawn_mode`、bool 取 `true`/`false`）；
7. `term_interactive=true` 时 `spawn_mode` 必须为 `pty`；**组合校验按平台展开**：默认组合与每个配置了 `spawn_mode`/`term_interactive` 变体的平台分别校验（FR-2.13）；
8. 平台维度（FR-2.13）：`platforms` 名字 slug 合法、不与内置冲突、家族枚举合法；`current_platform` 必须已注册；变体键后缀与 env `@平台` 标签须为注册表成员（或 `all`）；默认 `command` 必填，`command.<平台>` 非空可切分；**当前平台**生效表 env 完整校验（含系统环境引用），**其他平台**的带标签条目仅做结构性校验（键名、NUL、同标签重复键；`${VAR}` 引用可能是目标平台系统变量，本机放行，留待目标平台加载/启动时校验）；
9. 未知键报错（见 §1）。

## 8. 热加载行为（FR-2.6 / HLD RD-4）

- `notify` 监听文件变更（去抖 500 ms）→ 重新解析并校验 → 与运行态 diff → `ConfigDelta { added, removed, changed }` 串行应用。
- **校验失败**：丢弃本次变更（沿用上一次有效配置），托盘通知 + 设置窗口横幅提示错误明细。
- **added**：新服务出现在菜单；不因热载自动启动（`autostart` 仅指应用启动时），需手动或 CLI 启动。
- **removed**：运行中的服务按 FR-3.3 流程优雅停止后移出菜单。
- **changed**：判定字段 = `command` / `directory` / `environment` / `spawn_mode` / `shell` / `autorestart` / `startsecs` / `stopwaitsecs` / `stdout_logfile*` / `logfile_compress` / `matcher.*` → 命中则重启该服务使配置生效；仅 `name` / `autostart` 变更即时生效、不重启。未受影响的服务一律不重启。
- **平台变体（FR-2.13）**：命令行 / 工作目录 / 捕获模式 / 交互终端 / 环境变量按「各自解析链解析后的生效值」比较（变更前配置按旧链、变更后按新链）——改其他平台的变体或未命中本机标签的环境变量行不触发重启；`current_platform` / `platforms` 变更通过生效值变化自然反映。
- CLI 侧提供手动 `reread`（查看差异）与 `update`（应用差异），语义同上（见 [CLI-SPEC](./CLI-SPEC.md)）。

## 9. 与原 TOML 草案（HLD v0.1 §3.1）的差异与迁移

| TOML 草案 | 本规范（INI） |
|-----------|---------------|
| 文件 `config.toml`，`[[services]]` 数组，`id` 是字段 | 文件 `config.ini`，`[program:<id>]` 分节，id 取自节名 |
| `command` + `args: Vec<String>` 两字段 | `command` 单行含全部参数（§3.4 切分规则） |
| `env: Vec<EnvEntry { key, value, expand }>` | `environment=K="v", K2="!v2"`（`!` 前缀 = expand） |
| `stop_timeout_secs` | `stopwaitsecs` |
| `log: LogPolicy { path, max_bytes, max_files, compress }` | `stdout_logfile` / `stdout_logfile_maxbytes` / `stdout_logfile_backups` / `logfile_compress` |
| `matchers: Vec<MatcherConfig>` | `matcher.N.pattern / template / max_items` |
| `autorestart: No \| OnFailure \| Always`（Rust 枚举） | 字符串枚举 `no \| on-failure \| always` |

- **内存语义模型不变**：`ServiceConfig` 等结构体保留，仅文件编解码层从 TOML 换为 INI 映射（对照表即上表）；GUI 内部仍以"程序 + 参数数组、env 有序表"编辑，保存时序列化为本规范。
- MVP 无存量用户，不提供自动迁移工具；若启动时检测到旧 `config.toml`，提示一次性手工迁移（`traysvctl migrate` 列为 P2）。
- 与 Python `shlex` 的差异（§3.4 第 3 条）是唯一有意的行为偏离，目的是让 Windows 路径免转义；Unix 路径不受影响。
