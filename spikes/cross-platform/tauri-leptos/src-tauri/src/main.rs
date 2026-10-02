// 在 release 模式下隐藏 Windows 控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![greet])
        .setup(|app| {
            let handle = app.handle();

            // 验证目标 2：托盘图标 + 菜单（本产品核心交互入口）
            // 单击弹菜单（SRS 决策：不做左右键区分，三平台统一）
            let quit = MenuItem::with_id(handle, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(handle, &[&quit])?;

            let mut builder = TrayIconBuilder::with_id("main")
                .tooltip("Tauri+Leptos Spike")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    if event.id().as_ref() == "quit" {
                        app.exit(0);
                    }
                });

            // bundle 未配置图标时 default_window_icon 为 None
            if let Some(icon) = app.default_window_icon() {
                builder = builder.icon(icon.clone());
            }

            builder.build(app)?;
            println!("[spike] 托盘创建成功");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("tauri 运行失败");
}

/// 前端通过 window.__TAURI__.core.invoke("greet", { name }) 调用
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {name}!（来自 Rust 后端，前端是 Leptos/WASM）")
}
