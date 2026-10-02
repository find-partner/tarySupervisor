# Spikes（技术验证）

对应 [HLD §9 风险登记册](../docs/HLD.md) 的两个立项前验证项。按平台独立分目录，Windows 项请在 Windows 机器上检出后运行。

## 目录结构

| 目录 | 平台 | 验证目标 | 状态 |
|------|------|---------|------|
| `cross-platform/tauri-leptos/` | macOS / Linux / Windows | ① Tauri v2 + Leptos（Rust→WASM，零 Node）组合可编译可运行；② `withGlobalTauri` 下前端经 `window.__TAURI__.core.invoke` 调通 Rust command；③ `tauri::tray` 托盘 + 动态菜单创建 | 本机已验证 |
| `windows/job-object-kill-tree/` | 仅 Windows | Job Object（KILL_ON_JOB_CLOSE）对"cmd → cmd → ping"三层进程链整树终止（SRS FR-3.3 / HLD D-5 的核心机制） | 代码就绪，待 Windows 机器运行 |

## Spike 1：Tauri v2 + Leptos（跨平台）

前置：Rust + `rustup target add wasm32-unknown-unknown` + `cargo install trunk`。

```bash
cd spikes/cross-platform/tauri-leptos

# 终端 A：启动前端（Rust→WASM，无 Node.js）
trunk serve

# 终端 B：启动 Tauri 桌面壳
cd src-tauri && cargo run
```

验证点：
1. 窗口打开，UI 由 Leptos 渲染（非 HTML 模板）。
2. 输入名字点"调用 Rust 后端 greet"，显示后端返回消息 => IPC 链路通。
3. 托盘区出现图标，左键弹出菜单且含"退出"，点击可退出 => 托盘链路通。
4. 终端打印 `[spike] 托盘创建成功`。

## Spike 2：Windows Job Object 树杀（Windows 机器运行）

前置：Rust（MSVC 工具链）。

```bash
cd spikes/windows/job-object-kill-tree
cargo run --release
```

预期末尾输出 `[RESULT] PASS`（终止前 ping.exe ≥1 个，终止后 = 0）。

> 原理：`CREATE_SUSPENDED` 创建 → 绑定 Job Object → `ResumeThread`，保证子进程无逃逸窗口；`TerminateJobObject` 覆盖 cmd/孙进程全部成员。对应产品实现中"停止服务"的强杀兜底路径。
