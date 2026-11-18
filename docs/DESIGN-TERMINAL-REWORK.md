# 打开终端重构设计（服务级系统终端入口 · 工作目录语义 · 托盘菜单显隐开关）

| 项目 | 内容 |
|------|------|
| 版本 | v0.1（**已实现**，实现期校正两处：FR-1.4 追加项为 ⑦（⑥ 已被"编辑"占用）；SYSTEM-TEST 另加 ST-29 左栏旧入口移除验收） |
| 日期 | 2026-11-18 |
| 依据 | [SRS](./SRS.md) FR-2.16 / FR-1.4，[HLD](./HLD.md) D-20，[CONFIG-SPEC](./CONFIG-SPEC.md) §5 |
| 状态 | 已实现（SRS v0.6 FR-2.17 / HLD D-21 已落版；ST-28/29 手测用例见 SYSTEM-TEST） |

---

## 1. 背景与问题

FR-2.16 / HLD D-20 首版把「打开系统终端」做成了**全局入口**：

1. **设置页左栏**（`settings/mod.rs`，`open_config_terminal`）：在系统终端打开**应用配置目录**（`~/.traysupervisor`），注释明确"不依赖选中服务"；
2. **日志页工具栏**（`log_page.rs`，`open_log_terminal`）：打开该服务的**日志目录**。

实际使用反馈：终端入口的真实诉求是**针对某一个服务**——直接落到该服务的工作目录，便于手工执行命令、排查环境问题。全局"打开配置目录"按钮位置不对、语义不对；而托盘菜单里已有的「终端视图」是**应用内**的 PTY/文本只读视图（看输出用），与"在系统终端 App 中打开工作目录"（动手操作用）是两个动作，后者目前没有任何入口。

## 2. 目标与非目标

### 目标

1. **服务级语义**：新增"打开终端"动作 = 在系统终端 App 中打开**该服务当前平台生效的工作目录**（解析语义与服务 spawn 使用的 `ServiceConfig::resolve_for()` 完全一致）。
2. **托盘菜单显隐开关**：新增每服务配置键 `sys_term_menu`，控制该服务的托盘子菜单是否出现「打开终端」项。
3. **编辑页常驻入口**：编辑页服务操作栏（FR-2.11，与托盘对等原则）新增常驻「打开终端」按钮，**不受开关控制**（保证可发现性）。

### 非目标（本期不做）

- 不做终端 App 选择器（固定走 core `terminal` 模块的平台映射，D-20 决策不变）；
- 不做全局 `[traysupervisor]` 级默认开关（每服务独立声明）；
- 日志页「打开终端」保留不动（"日志目录"语义独立且合理）；
- `sys_term_menu` **不支持平台变体**：菜单显隐是本机展示偏好，不属于"基础配置"启动语义，`is_variant_key` 白名单（`program.rs:27`）不扩充。

## 3. 入口与语义变化总表

| 入口 | 现状 | 变化 |
|------|------|------|
| 设置页左栏「打开终端」 | `open_config_terminal`，打开应用配置目录 | **移除**（按钮 + 命令一并删除，语义被服务级入口取代） |
| 日志页工具栏「打开终端」 | `open_log_terminal`，打开该服务日志目录 | **保留不变** |
| 编辑页服务操作栏 | 无 | **新增「打开终端」按钮**：打开当前选中服务的生效工作目录，常驻 |
| 托盘每服务子菜单 | 无（只有「终端视图」） | **新增「打开终端」项**（`svc:<id>:systerm`），受 `sys_term_menu` 控制显隐 |

**目标目录解析**（core 既有机制，无新增逻辑）：

1. 取 `ServiceBrief.config`，调用 `resolve_for(&PlatformRegistry)` 得到当前平台生效配置（`config/mod.rs:255`，命令层已有同款调用：`commands.rs:304`）；
2. 目标目录 = `eff.cwd`（INI `directory` / `directory.<平台>`，解析期已展开 `~`，`program.rs:60,142`）；
3. `cwd` 为 `None`（未配置 directory）→ **回退 `config_dir()`**——沿用 `open_log_terminal` 的"终端侧总能落在一个可用的目录"哲学（`commands.rs:584` 注释），不报错；
4. core `terminal::open_in_terminal` 内部 `create_dir_all`（`terminal.rs:61`）：目录不存在时自动创建后打开，行为可接受，不额外校验。

## 4. 新配置键 `sys_term_menu`（CONFIG-SPEC §5 扩展）

| 键 | 节 | 类型 | 默认 | 说明 |
|----|----|------|------|------|
| `sys_term_menu` | `[program:*]` | bool | `false` | `true` 时该服务的托盘子菜单显示「打开终端」项；编辑页操作栏按钮不受此键影响（常驻） |

- **默认 `false`（opt-in）**：托盘子菜单已有状态行 / 启动 / 停止 / 重启 / 查看日志 / 终端视图 / Matcher 快捷项，一级菜单空间宝贵，终端操作属低频动作；需要时在编辑页勾选开启。与 Matcher 快捷项同为 opt-in 模式。
- 解析：`parse_bool`，非法值报错文案同其他 bool 键（`[{section}] sys_term_menu：…`）；加入 `KNOWN_KEYS`（`program.rs:212`）。
- `ServiceConfig` 新增字段 `sys_term_menu: bool`（默认 false），`upsert` 序列化 / load-save roundtrip 自然保留（`config/mod.rs:901` 的变更比对无需纳入——它不影响 spawn 行为，但 roundtrip 必须保留该键）。
- 编辑页「① 基础配置」页新增复选框，label「托盘菜单显示"打开终端"」，与 `term_interactive` 同页（同为服务终端相关选项）；**不提供平台变体编辑**（与 `shell` 同待遇）。
- DTO 双侧手工镜像（DEV-CONVENTIONS §2）：`src/dto.rs` `ServiceConfigOut` 新增字段，前端侧 `#[serde(default)]`。

## 5. 托盘菜单（tray.rs，FR-1.4 扩展）

- `build_menu`（`tray.rs:144`）每服务子菜单：「终端视图」项之后、Matcher 分隔线之前，**仅当 `brief.config.sys_term_menu == true`** 时创建：

  ```
  ● RUNNING (PID 1234)
  ─────────────
  启动 / 停止 / 重启
  ─────────────
  查看日志
  终端视图          ← 应用内（现有）
  打开终端          ← 新增（系统终端 App，svc:<id>:systerm）
  ─────────────
  🌐 Matcher 快捷项…
  ```

- **ID 稳定编码**（FR-1.7）：`svc:<id>:systerm`；`handle_menu_event` 新增分支 → 调 `open_service_terminal(id)`。
- **显隐变化走既有重建路径**：开关是配置键，保存 / 热更后 `ConfigChanged` → `rebuild_all` 全量重建（`tray.rs:399`），无需新增就地更新逻辑；该项为静态项，不随 StateChanged 变化。
- 文案「打开终端」与「终端视图」并存：子菜单已含服务名语境，且编辑页操作栏用同一对文案，两侧保持一致。

## 6. 命令层（src-tauri）

```rust
/// 服务级"打开终端"（本设计 §3）：在系统终端打开该服务当前平台生效工作目录；
/// 未配置 directory 回退应用配置目录——终端侧总能落在可用目录。
#[tauri::command]
pub fn open_service_terminal(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let reg = registry_of(&state);
    let dir = state
        .mgr
        .list()
        .into_iter()
        .find(|b| b.config.id == id)
        .map(|b| b.config.resolve_for(&reg))
        .and_then(|eff| eff.cwd)
        .unwrap_or_else(traysupervisor_core::config_dir);
    traysupervisor_core::terminal::open_in_terminal(&dir)
}
```

- 删除 `open_config_terminal`（命令 + `lib.rs` 注册 + 设置页调用点）；`open_log_terminal` 保留。
- core `terminal` 模块（D-20）零改动，纯函数单测不动。

## 7. 前端（src/，Leptos WASM）

| 文件 | 变化 |
|------|------|
| `dto.rs` | `ServiceConfigOut` 新增 `sys_term_menu: bool`（`#[serde(default)]`） |
| `settings/form.rs` | `FormState` 新增字段；`blank_form` / `form_from_brief` / `input_from_form`（含 `merged_for_save`）同步；「基础配置」页复选框（沿用 `term_interactive` 复选框写法） |
| `settings/ctrlbar.rs` | 操作栏新增「打开终端」按钮：`bridge::invoke_unit_res("open_service_terminal", id)`，排在「终端视图」之后 |
| `settings/mod.rs` | 删除左栏按钮（`mod.rs:338`）与 `open_term` 闭包（`mod.rs:111`） |
| `log_page.rs` | 不动 |

## 8. 文档同步（实现完成后落版）

| 文档 | 变化 |
|------|------|
| SRS | **FR-2.16 改写**：删除左栏全局按钮描述，保留日志页部分；**新增 FR-2.17**：服务级「打开终端」（目录语义 = 平台生效 directory、未配置回退配置目录；托盘项受 `sys_term_menu` 控制；编辑页操作栏常驻按钮）；**FR-1.4** 子菜单清单追加「⑥ 打开终端（`sys_term_menu=true` 时显示）」 |
| HLD | 新增 **D-21**：服务级终端入口——目录解析复用 `resolve_for`、回退 `config_dir`；托盘项随 ConfigChanged 重建、ID 编码 `svc:<id>:systerm`；D-20 保持不变（core 零改动） |
| CONFIG-SPEC | §5 表新增 `sys_term_menu`（默认 false、不支持平台变体、未知键校验生效）；保存序列化节如涉及同步 |
| SYSTEM-TEST | 新增手测用例：① 开关开 → 托盘出现/关 → 消失（含热更）；② 点击后终端落在工作目录；③ `directory.<平台>` 变体生效；④ 未配置 `directory` 回退配置目录 |

版本号：随下一版本计划（当前已发布 0.6.0）。

## 9. 验收标准

1. `cargo test`（workspace）全绿；core 新增单测：`sys_term_menu` 解析默认值 / 显式值 / 非法值报错 / load-save roundtrip 保留。
2. 手测（§8 SYSTEM-TEST 用例）通过：托盘显隐、目录落点、平台变体、回退路径。
3. 编辑页操作栏对任意选中服务显示「打开终端」且可用；左栏全局按钮与 `open_config_terminal` 命令已删除。
4. DTO 双侧同步（`cargo check` + 前端 `trunk build`/编译通过），遵守 DEV-CONVENTIONS §2（前端 `#[serde(default)]`）。

## 10. 执行说明（定时任务约束）

- 实施顺序：core（`config` 解析 + `ServiceConfig` 字段 + 单测）→ 命令层（`commands.rs` / `lib.rs`）→ 托盘（`tray.rs`）→ 前端（`dto.rs` / `form.rs` / `ctrlbar.rs` / `mod.rs`）→ 文档（§8）→ 测试（§9）。
- 注释风格随既有代码：模块头注明对应 FR / 设计编号（FR-2.17 / D-21）。
- **不做 git commit**：完成后保持工作区改动并汇报改动清单与测试结果，由用户确认后提交。
