//! 「基本设置」标签页（FR-2.8 页 1）：ID / 命令行 / 启动方式等核心字段。

use leptos::*;

use super::form::{set_form_value, FormState};

pub fn basic_panel(
    form: RwSignal<FormState>,
    dirty: RwSignal<Vec<bool>>,
    is_new: bool,
) -> impl IntoView {
    let mark0 = move || dirty.update(|d| d[0] = true);
    view! {
        <div class="field-grid">
            <label>"ID（唯一）"</label>
            <input type="text" prop:value=move || form.get().id
                readonly=!is_new
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.id = v);
                    mark0();
                } />
            <label>"显示名"</label>
            <input type="text" prop:value=move || form.get().name.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.name = v);
                    mark0();
                } />
            <label>"命令行"</label>
            <input type="text" placeholder=r#"python -m http.server 8000 --directory "C:\work\docs""#
                prop:value=move || form.get().command_line.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.command_line = v);
                    mark0();
                } />
            <span class="hint">"程序 + 参数，空格分隔，双引号包裹含空格部分；默认不经 shell（FR-3.6）"</span>
            <label>"经 shell 启动"</label>
            <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                <input type="checkbox" prop:checked=move || form.get().shell
                    on:change=move |e| {
                        let v = event_target_checked(&e);
                        set_form_value(form, move |f| f.shell = v);
                        mark0();
                    } />
                "经登录 shell（bash/zsh）启动，继承终端环境（PATH、nvm 等）；命令行按原样重组，不支持管道等 shell 语法"
            </label>
            <label>"工作目录"</label>
            <input type="text" placeholder="默认用户目录，支持 ~"
                prop:value=move || form.get().cwd.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.cwd = v);
                    mark0();
                } />
            <label>"启动捕获模式"</label>
            <select on:change=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.spawn_mode = v);
                    mark0();
                }
                prop:value=move || form.get().spawn_mode.clone()>
                <option value="Pipe">"管道（pipe）"</option>
                <option value="Pty">"伪终端（pty，启用终端视图）"</option>
            </select>
            <label>"交互终端"</label>
            <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                <input type="checkbox" prop:checked=move || form.get().term_interactive
                    on:change=move |e| {
                        let v = event_target_checked(&e);
                        set_form_value(form, move |f| f.term_interactive = v);
                        mark0();
                    } />
                "终端视图可写（键盘输入转发 stdin；需 pty 模式）"
            </label>
            <label>"autostart"</label>
            <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                <input type="checkbox" prop:checked=move || form.get().autostart
                    on:change=move |e| {
                        let v = event_target_checked(&e);
                        set_form_value(form, move |f| f.autostart = v);
                        mark0();
                    } />
                "应用启动时自动启动"
            </label>
            <label>"autorestart"</label>
            <select on:change=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.autorestart = v);
                    mark0();
                }
                prop:value=move || form.get().autorestart.clone()>
                <option value="No">"no（不自动重启）"</option>
                <option value="OnFailure">"on-failure（异常退出时重启）"</option>
                <option value="Always">"always（总是重启）"</option>
            </select>
            <label>"startsecs"</label>
            <input type="number" min="0" prop:value=move || form.get().startsecs.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.startsecs = v);
                    mark0();
                } />
            <label>"startretries"</label>
            <input type="number" min="0" max="10" prop:value=move || form.get().startretries.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.startretries = v);
                    mark0();
                } />
            <label>"stopwaitsecs"</label>
            <input type="number" min="1" prop:value=move || form.get().stopwaitsecs.clone()
                on:input=move |e| {
                    let v = event_target_value(&e);
                    set_form_value(form, move |f| f.stopwaitsecs = v);
                    mark0();
                } />
            <label>"redirect_stderr"</label>
            <label style="display:flex;gap:6px;align-items:center;color:var(--fg)">
                <input type="checkbox" prop:checked=move || form.get().redirect_stderr
                    on:change=move |e| {
                        let v = event_target_checked(&e);
                        set_form_value(form, move |f| f.redirect_stderr = v);
                        mark0();
                    } />
                "stderr 并入 stdout 单流"
            </label>
        </div>
    }
}
