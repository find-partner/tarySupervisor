# 服务配置重组设计（粘贴解析 · 配置分组 · 平台体系与基础配置变体）

| 项目 | 内容 |
|------|------|
| 版本 | v0.5（**已实现**，实现期校正两处，见 §8；SRS/HLD/CONFIG-SPEC v0.5 已落版） |
| 日期 | 2026-10-06 |
| 依据 | [SRS](./SRS.md) v0.4 FR-2 / FR-2A，[CONFIG-SPEC](./CONFIG-SPEC.md) v0.3 §2/§3 |
| 状态 | 已实现（分支 feat/platform-variant-config；ST-23~25 手测用例见 SYSTEM-TEST v0.3） |
| 修订 | v0.2：平台变体扩展为四字段；spawn_mode、term_interactive 移入「基础配置」页；单配置文件跨平台同步。<br>v0.3：环境变量平台化纳入本期。<br>v0.4：**平台升级为体系**——① 环境变量改为**行级平台标签**（单表 + `@platform` 后缀，取代 v0.3 的 `environment.<os>` 分表）；② **平台注册表**：内置 windows/macos/linux，全局配置可声明**自定义平台**；③ 全局配置可声明**当前所在平台**（自定义平台无法自动检测，需显式选择） |

---

## 1. 背景与目标

1. **命令行粘贴解析**：自动识别粘贴整行命令中的前缀环境变量（`KEY=VALUE`）、程序与启动参数，分别填入对应配置（新增 FR-2.12）。
2. **配置界面按关注点分组**：环境变量、命令行等高频"基础配置"与低频的"启动策略""日志"分页（修订 FR-2.8/2.9）。
3. **平台体系**（新增 FR-2.13）：
   - 平台默认有 **windows / macos / linux** 三个内置项（运行时自动检测）；
   - **全局配置可声明自定义平台**（如 `home-mac`、`office-win`），满足"多台同类机器、每台命令不同"的场景；
   - **全局配置可选择当前所在平台**——自定义平台无法自动检测，必须由用户声明"这台机器是谁"；
   - **环境变量逐行打平台标签**：默认"全部平台"，也可只给特定平台；同一张表里共享变量与平台变量一目了然；
   - 其余基础字段（命令行、工作目录、捕获模式、交互终端）按 `键.<平台>` 变体覆盖；
   - **配置文件仍是单份 INI**，备份同步到任何机器后各平台各取所需。

非目标（本期不做）：shell 语法（管道、`&&`）支持（维持 FR-3.6 禁令）；全局配置的图形界面（自定义平台/当前平台本期在配置文件中声明，热加载生效；设置 UI 列为跟进项，见 §7）。

## 2. 参数分类总表（哪些参数在哪些分类下、哪些支持平台变体）

标签页从「基本设置 / 环境变量 / 日志 / 快捷项」调整为：
**① 基础配置 / ② 启动策略 / ③ 日志 / ④ 快捷项**。

**只有 ① 基础配置页的字段支持平台变体**（表中标 ◆）；②③④ 全平台共用。

### ① 基础配置 —— "这个服务是什么、怎么拉起来"（高频编辑区，支持平台变体）

| 字段（INI 键） | 现属标签页 | 平台变体形式 | 变化 |
|----------------|-----------|-------------|------|
| name（显示名） | 基本设置 | —（共用） | 移入本页 |
| command（命令行，程序+参数）◆ | 基本设置 | `command.<平台>` | 移入本页；**新增粘贴解析**（§3） |
| directory（工作目录）◆ | 基本设置 | `directory.<平台>` | 移入本页（路径写法因平台而异） |
| spawn_mode（pipe/pty）◆ | 基本设置 | `spawn_mode.<平台>` | **移入本页**（捕获方式属启动语义；pty 可用性因平台而异） |
| term_interactive（交互终端）◆ | 基本设置 | `term_interactive.<平台>` | **移入本页**（依赖 pty，随 spawn_mode 同页配置、同规则校验） |
| shell（经登录 shell 启动） | 基本设置 | —（共用；Unix=`$SHELL -ilc`、Windows=`%COMSPEC%`，实现层已双平台处理） | 移入本页 |
| environment（环境变量 KV 表 + 展开预览）◆ | 环境变量 | **行级平台标签**（单表，每行可标 `@平台`，默认全部平台） | **整页并入**本页（原独立标签页取消）；生效表按标签过滤合并（§4.3） |

### ② 启动策略（低频，配置一次少改，全平台共用）

autostart / autorestart / startsecs / startretries / stopwaitsecs —— 自基本设置移入。

### ③ 日志（全平台共用）

redirect_stderr（自基本设置移入，决定日志里有几路流）+ stdout_logfile / _maxbytes / _backups / logfile_compress（不变，默认路径含 `~` 本身可移植）。

### ④ 快捷项（Matcher，全平台共用）

matcher.N.* 不变（新建仍预填内置 URL 规则，FR-5.3）。

> 每页独立保存（FR-2.8）与"未保存"标记机制不变，仅字段归属调整（`merged_for_save` 同步改写）。

## 3. 命令行粘贴解析（新增 FR-2.12）

### 3.1 规则

沿用 CONFIG-SPEC §3.4 切分规则（空白分 token、双引号成 token、`\` 最小转义）先切 token，再按 Unix 惯例识别**前缀环境变量**：

1. 从第一个 token 起扫描：token 形如 `KEY=VALUE` 且 `KEY` 合法（`[A-Za-z_][A-Za-z0-9_]*`）→ 计入**前缀环境变量候选**；
2. 遇到第一个不符合该形态的 token 即为**程序**；其后所有 token 一律为**参数**（即使形如 `A=B`，如 `--port=1123`）；
3. 整行全部是 `KEY=VALUE`（没有程序）→ 报错「缺少命令——请检查粘贴内容」；
4. 没有任何前缀环境变量 → 行为与现状完全一致（整行即命令行）；
5. 重复 `KEY` 后者覆盖前者（与 shell 语义一致）；
6. 识别出的环境变量条目 `expand=false`、**平台标签 = 全部平台**（字面量直传，FR-2.3 默认语义）；值中的引号已由 token 切分剥除；
7. 解析结果就地填入表单：env 条目追加进环境变量表格、命令行输入框改写为「程序 + 参数」；后续校验、保存、INI 落盘走既有链路。

### 3.2 示例

| 粘贴内容 | 环境变量 | 命令行 |
|----------|---------|--------|
| `ENABLE_CORS=true USE_PROXY=true PROXY_URL=http://127.0.0.1:7890 open-websearch --port=1123` | ENABLE_CORS=true、USE_PROXY=true、PROXY_URL=http://127.0.0.1:7890（3 条，全部平台） | `open-websearch --port=1123` |
| `FOO="hello world" python -m http.server 8000` | FOO=hello world | `python -m http.server 8000` |
| `python FOO=bar x` | （无——`FOO=bar` 在程序之后，是参数） | `python FOO=bar x` |
| `FOO=bar` | 报错：缺少命令 | — |

### 3.3 交互

- **自动**：命令行输入框 `paste` 事件时，若解析出 ≥1 个前缀环境变量且有程序 → 自动拆分填入（env 表追加行 + 提示「已识别 N 个前缀环境变量」）；否则原样插入。
- **手动兜底**：输入框旁「解析」按钮对当前输入框内容重新解析。
- 新建与编辑均可用；**只作用于默认命令行**（平台变体命令行不提供粘贴解析）。

### 3.4 实现位置

core 纯函数 `split_env_prefixed_line(line) -> Result<(Vec<EnvEntry>, String), String>`，复用 `cmdline.rs` tokenizer；经 Tauri command `parse_pasted_line` 暴露给前端。core 单测覆盖 §3.2 全部示例与引号/转义边界。

## 4. 平台体系与跨平台基础配置变体（新增 FR-2.13）

### 4.1 平台注册表与当前平台（CONFIG-SPEC §2 全局节增补）

```ini
[traysupervisor]
log_lines=2000
notify_on_failure=true
; ---- 平台注册表（可选）----
; 内置：windows / macos / linux（运行时按 std::env::consts::OS 自动检测）
; 自定义平台：名字为 slug（[a-z0-9-_]），可选声明 OS 家族（:macos / :windows / :linux）
platforms=home-mac:macos, office-win:windows, build-box
; 当前所在平台（可选；空 = 按内置自动检测）
current_platform=home-mac
```

- **自定义平台**：`platforms=` 逗号分隔声明；`:家族` 后缀可选，声明后该平台未命中变体时回退到家族平台的变体（如 `home-mac` 未配的变体回落到 `macos` 的变体）；与内置名同名 → 校验错误。
- **当前平台**：`current_platform=` 指明"这台机器是谁"，取值须为内置或已声明平台（否则校验错误）。**内置三平台无需设置**（自动检测）；自定义平台必须设置，否则其变体永不生效。
- 生效解析链（按序取第一个命中，均未命中回退默认值）：
  1. `current_platform` 本身；
  2. 其 OS 家族（仅自定义平台且声明了家族时）；
  3. 自动检测的内置 OS（仅当链中尚未含家族时附加；未设 current_platform 时链即=[自动检测 OS]）。
- **同步注意**：`current_platform` 属于"机器本地状态"，随配置文件一起同步——换机后需改一次这一行。仅用内置平台时留空即可（全自动），文档与 UI 提示会明确这一点。

### 4.2 服务级变体：单配置文件，读时解析

```ini
[program:open-websearch]
name=Open WebSearch
; ---- 默认值（任何平台未命中变体时使用）----
command=npx -y open-websearch@latest --port=1123   ; 必填
directory=~/work
spawn_mode=pty
term_interactive=true
environment=PORT="3000", LOG_DIR="~/logs", LOG_DIR="!${LOCALAPPDATA}\logs"@windows, PATH_EXTRA="/opt/homebrew/bin"@home-mac

; ---- 平台变体（全部可选；<平台> 取自注册表）----
command.macos=/opt/homebrew/bin/open-websearch --port=1123
command.office-win=C:\tools\open-websearch.exe --port 1123
directory.windows=C:\work
spawn_mode.windows=pipe
term_interactive.windows=false
```

- 变体键后缀 ∈ 注册表（内置 + 自定义）；未声明平台的后缀按**未知键**校验错误。
- 各变体键值语义与对应基础键完全一致（`command` 按 §3.4 切分；`directory` 支持 `~`；枚举/bool 同基础键）。

**内存模型**：

```rust
/// 环境变量条目：新增行级平台标签；None = 全部平台
pub struct EnvEntry { pub key: String, pub value: String, pub expand: bool, pub platform: Option<String> }

/// 标量字段变体（命令行/工作目录/捕获模式/交互终端）；None = 该平台用默认值
#[derive(Default)]
pub struct PlatformVariant {
    pub command_line: Option<String>,
    pub directory: Option<PathBuf>,
    pub spawn_mode: Option<SpawnMode>,
    pub term_interactive: Option<bool>,
}

// ServiceConfig 新增
pub platform: BTreeMap<String, PlatformVariant>,
```

- 新增解析方法：`resolve_for(registry) -> ResolvedLaunch { command, args, cwd, env, spawn_mode, term_interactive }`——标量字段沿 §4.1 解析链逐级取变体、回退默认；env 按 §4.3 过滤合并；
- spawn、`list_services` 回显的 `command_line`、托盘/CLI 展示、终端视图可用性判断、env「预览」，统一采用当前平台解析结果；
- **存盘无损**：`to_ini_string` 原样写出默认值 + 全部平台变体 + 带标签的环境变量行。

### 4.3 环境变量的行级标签与生效合并

同一张 `environment=` 表里，共享变量与平台变量一目了然；**标签写在条目引号值之后**：`KEY="value"@平台`，无标签/`@all` = 全部平台：

1. **生效表 = 标签命中的条目**（标签 ∈ 解析链，或全部平台），**顺序保持书写顺序**：
   - **同名键，平台标签命中 vs 全部平台** → 平台条目**就地覆盖**全部平台条目（value 与 expand 均以平台条目为准、位置不变）——后续条目展开 `${VAR}` 取到覆盖后的值，与最终注入一致；
   - 同名键多个平台条目均命中（如重复书写）→ 书写顺序后者覆盖前者；
   - 标签未命中的条目**整体排除**（不注入、不参与展开）。
2. **`${VAR}` 展开规则不变**（FR-2.3）：查排在其前面的条目，再查系统环境；合并（过滤+覆盖）发生在展开之前。
3. **示例**（Windows 机，`LOCALAPPDATA=C:\Users\me\AppData\Local`，解析链=[home-mac, macos] 的机器则命中 `@home-mac` 条目）：

   ```ini
   environment=PORT="3000", LOG_DIR="~/logs", LOG_DIR="!${LOCALAPPDATA}\logs"@windows, EXTRA="只在 home-mac 生效"@home-mac
   ```
   Windows（无 current_platform，链=[windows]）：`PORT=3000`、`LOG_DIR=C:\Users\me\AppData\Local\logs`（expand=true）；macOS 自动检测（链=[macos]）：`PORT=3000`、`LOG_DIR=~/logs`（字面量）；home-mac（链=[home-mac, macos]）：`LOG_DIR=~/logs` + `EXTRA`。

### 4.4 校验（CONFIG-SPEC §7 追加）

- 全局节：`platforms` 名字 slug 合法、不与内置冲突；家族枚举合法；`current_platform` ∈ 内置 ∪ 已声明；
- 变体键后缀 ∈ 注册表；`command` 默认值仍必填；`command.<平台>` 非空可切分；枚举/bool 合法；
- **组合校验按平台展开**：对默认组合及每个"配置了 spawn_mode 或 term_interactive 变体"的平台，`term_interactive=true` 必须落在 `spawn_mode=pty` 上（如 `term_interactive=true` + `spawn_mode.windows=pipe` 为校验错误，须成对覆盖）；
- **环境变量分两档**：
  - **当前平台**生效表：完整校验（UTF-8、开启替换的 `${VAR}` 在"生效表 + 本机系统环境"可解析、无环），不通过阻止保存/加载；
  - **其他平台**：对每个被标签引用的平台构造其生效表，只做本表内可判定项（UTF-8、键名、表内可解析引用、无环）；引用目标平台系统变量的条目本机无法核实——放行，留到目标平台加载/启动时校验。保证在 macOS 上可放心编辑 `@windows` 条目。

### 4.5 热加载与保存的 changed 判定（CONFIG-SPEC §8 追加）

- `diff_configs` 对基础字段**比较"按各自解析链解析后的生效值"**（含合并后的 env）：改其他平台的变体或未命中标签的环境变量行 → 本机生效值不变 → 不重启；改默认值或命中本机的变体/标签 → hard change（运行中服务提示需重启，同 FR-2.10）；
- 全局节 `current_platform` / `platforms` 变更会改变解析链：diff 用"变更前配置按旧链、变更后配置按新链"的生效值比较，因此 current_platform 变更导致的生效值变化会被识别为 hard change，而仅增删无关自定义平台不触发重启。

### 4.6 兼容性声明（DEV-CONVENTIONS §1 同款）

`platforms` / `current_platform` / `command.<平台>` / `directory.<平台>` / `spawn_mode.<平台>` / `term_interactive.<平台>` 及 env 行级 `@平台` 标签均为新增语法：旧配置文件（无这些键、env 无标签）解析不受影响（无标签 = 全部平台 = 现行为）；含新语法的配置文件在旧版本应用上会因「未知键/解析错误」被拒绝，不做兼容。v0.3 草案中的 `environment.<os>` 分表方案**废弃未实现**，无迁移负担。

### 4.7 UI（「基础配置」页内）

- 命令行（默认）下方增加**「平台变体」**小节：子标签 **默认 / <注册表平台逐个列出>**。
  - 「默认」子页：命令行（含粘贴解析）、工作目录、捕获模式、交互终端、经 shell 启动、环境变量表（主体）；
  - 平台子页：**命令行 / 工作目录 / 捕获模式 / 交互终端**覆盖输入（留空 = 跟随默认，占位文本显示回退值；含家族回退的显示"未配置时回退 <家族/默认>"）；
  - 本机命中链上的平台子标签带「本机」角标；页内提示当前解析链（如"当前平台：home-mac（→ macos）"）。
- **环境变量表新增「平台」列**：每行一个下拉（全部平台 + 注册表平台），默认"全部平台"；选项随全局注册表动态生成。
- env「预览」按钮展示当前平台生效表 + 展开结果；保存走同一 `upsert_service` 校验链。

## 5. 向导步骤调整（FR-2.9 修订）

| 步骤 | 内容 | 必填 |
|------|------|------|
| ① 基础配置 | ID / 显示名 / 命令行（粘贴解析）/ 工作目录 / shell / 捕获模式 / 交互终端 / 环境变量表（含平台列） | ID、命令行 |
| ② 启动策略 | autostart / autorestart / startsecs / startretries / stopwaitsecs | 可跳过 |
| ③ 日志 | redirect_stderr / 文件路径 / 轮转 / 压缩 | 可跳过 |
| ④ 快捷项 | Matcher（预填内置 URL 规则） | 可跳过 |

> 平台变体不进向导：新建时通常只配默认值，变体在保存后于编辑页「平台变体」补充。

## 6. 实现拆解（确认后执行）

| # | 模块 | 内容 |
|---|------|------|
| 1 | `crates/core/config/cmdline.rs` | `split_env_prefixed_line` + 单测（§3.2 全覆盖） |
| 2 | `crates/core/config/mod.rs` | `PlatformRegistry`（内置 + 全局声明、家族、解析链 `chain_for(current)`）；`EnvEntry.platform`；`PlatformVariant`、`ServiceConfig.platform`；`resolve_for(registry)` 与 env 标签过滤合并（§4.3）；`diff_configs` 按 §4.5 |
| 3 | `crates/core/config/ini/` | 全局节解析 `platforms` / `current_platform` + 校验；`program.rs` 解析 `键.<平台>` 变体（后缀白名单 = 注册表）与 env 行 `@平台` 标签（`envstr.rs` 扩展）；`to_ini_string` 回写；`tests.rs` 往返/合并/解析链/diff/组合校验用例 |
| 4 | `src-tauri/commands.rs` | `ServiceInput.platform` 映射与校验；`parse_pasted_line` command；`ServiceBriefOut` 增平台变体与注册表回显、`command_line` 改为当前平台生效值；env 预览按当前平台生效表 |
| 5 | 前端 `src/` | `dto.rs` 镜像（EnvEntry.platform、注册表）；`form.rs` FormState/`merged_for_save` 重组；`basic.rs` 重写为「基础配置」（默认字段 + 平台变体子标签 + 粘贴解析 + env 表）；`env_panel` 加「平台」下拉列（选项来自注册表）；新增「启动策略」面板；`edit.rs`/`wizard.rs` 调整 |
| 6 | 文档落版 | SRS：FR-2.12/2.13、修订 FR-2.8/2.9；CONFIG-SPEC v0.5：§2 全局节新键、§3.6 粘贴解析、§3.7 平台体系与合并语义、§7/§8；HLD：§3.1 模型（PlatformRegistry/PlatformVariant/resolve_for）、§4 IPC；SYSTEM-TEST 补用例 |
| 7 | 测试 | core 单测（解析/合并/解析链/往返/diff/组合校验）；手测：粘贴示例、自定义平台声明与 current_platform 切换、跨平台变体回显与往返、"改其他平台不重启本机"、运行中服务重启提示 |

## 7. 决议与遗留

已确认：
1. 平台变体范围 = 基础配置页字段：命令行、工作目录、捕获模式、交互终端、环境变量（行级标签）；name/shell 共用；
2. spawn_mode、term_interactive 移入「基础配置」页；
3. 环境变量**行级平台标签**（默认全部平台 + 按平台标注），取代分平台 env 表；
4. 平台注册表：内置 windows/macos/linux + 全局配置声明自定义平台（可带 OS 家族回退）；
5. 全局配置 `current_platform` 选择当前所在平台（自定义平台必需；内置平台自动检测）；
6. 单配置文件 `config.ini` 跨平台同步（`current_platform` 为机器本地行，换机后改一次）。

遗留 / 待确认：
7. **自定义平台与当前平台的设置 UI**：本期在配置文件中声明（热加载生效），设置页只读展示当前解析链；独立的全局设置界面列为跟进项（P2）；
8. **粘贴解析自动触发**（paste 自动拆分 + 按钮兜底）——推荐自动；如担心误拆可改仅手动。

---

## 8. 实现期校正（v0.5）

实现过程中对 v0.4 的两处修正（均已按此实现并入 CONFIG-SPEC v0.5）：

1. **`ServiceBriefOut` 命令行回显提供两个值**：`command_line`（默认/基础命令行，编辑表单「默认」子页使用）与 `effective_command_line`（当前平台生效命令行，编辑页「本机实际执行」提示使用）。v0.4 的「`command_line` 改为当前 OS 生效值」会破坏表单回显语义——把变体值当默认值改回存盘，故弃用。对应地，`PlatformVariant` 内存中直接持有切分后的 `command` + `args`（解析期切分，`resolve_for` 无失败路径），序列化时经 `join_command_line` 还原为单行（roundtrip 有测试保证）。
2. **「其他平台」环境变量校验进一步简化为仅结构性校验**（键名、NUL、同标签重复键）：v0.4 设想的「表内可解析引用 + 无环」在本机无法与「引用目标平台系统变量」区分（如 `@windows` 条目引用 `${LOCALAPPDATA}`），会产生误报；引用可解析性统一放行，由目标平台加载/启动时的完整校验兜底（启动时展开失败已有报错路径）。

其余实现说明：监督任务收到的配置为**解析后**的生效配置（`create_service` / reload 注入），原始配置保留在 manager 供存盘与编辑回显；`diff_configs(old, new, old_reg, new_reg)` 新旧各自按其解析链比较生效值；GUI 粘贴解析经 Tauri command `parse_pasted_line` 调 core 纯函数，前端以「首 token 形如 `KEY=`」同步前置判断决定是否接管默认插入。
