//! 空态引导（FR-2.7）：无任何服务时的欢迎页。

use leptos::*;

pub fn empty_state() -> impl IntoView {
    view! {
        <div style="max-width:640px;margin:48px auto 0;padding:0 24px;text-align:left;">
            <h2 style="margin:0 0 10px;font-size:18px;">"欢迎使用 TraySupervisor"</h2>
            <p style="color:var(--muted);line-height:1.8;margin:0 0 18px;">
                "把你的命令行服务注册进来，由本工具作为子进程托管：从系统托盘启动 / 停止 / 重启，"
                "实时查看日志（可选按大小轮转的文件日志），在只读或交互终端中观察输出，"
                "并可用正则从日志中提取快捷链接（如 🌐 打开 http://localhost:3000）。"
            </p>
            <div class="section-title">"开始使用"</div>
            <ol style="line-height:2.1;color:var(--fg);margin:0 0 18px;padding-left:22px;">
                <li>"点击下方或左下角的 <b>「+ 新建服务」</b>，按向导填写命令行即可"</li>
                <li>"保存后从系统托盘菜单启动服务"</li>
                <li>"托盘菜单中查看状态、日志、终端视图与 Matcher 快捷链接"</li>
            </ol>
            <p class="hint" style="margin:0 0 16px;">"当前还没有任何服务配置——请新建配置，或编辑 ~/.traysupervisor/config.ini（支持热加载）。"</p>
        </div>
    }
}
