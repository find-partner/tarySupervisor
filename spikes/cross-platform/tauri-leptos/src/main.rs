use leptos::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(|| view! { <App /> });
}

/// 验证目标：Leptos(Rust→WASM) 渲染 UI，并通过 withGlobalTauri 暴露的
/// window.__TAURI__.core.invoke 调用 Rust 后端 command（全程无 Node.js）。
#[component]
fn App() -> impl IntoView {
    let (name, set_name) = create_signal("Leptos".to_string());
    let (reply, set_reply) = create_signal("（尚未调用后端）".to_string());

    let on_greet = move |_| {
        let n = name.get_untracked();
        spawn_local(async move {
            let msg = invoke_greet(&n).await;
            set_reply.set(msg);
        });
    };

    view! {
        <div style="font-family: sans-serif; padding: 16px; line-height: 1.6;">
            <h2>"Tauri v2 + Leptos Spike"</h2>
            <input
                value = move || name.get()
                on:input = move |e| set_name.set(event_target_value(&e))
                placeholder = "输入你的名字"
            />
            <button on:click = on_greet style="margin-left: 8px;">
                "调用 Rust 后端 greet"
            </button>
            <p><b>"后端返回："</b>{move || reply.get()}</p>
            <p style="color: gray; font-size: 12px;">
                "若上方显示后端消息，则验证通过：Leptos(WASM) → window.__TAURI__.core.invoke → Rust command"
            </p>
        </div>
    }
}

async fn invoke_greet(name: &str) -> String {
    match tauri_invoke("greet", name) {
        Ok(fut) => fut
            .await
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "调用失败：返回值不是字符串".to_string()),
        Err(e) => format!("调用失败：{e}"),
    }
}

/// 通过全局 API window.__TAURI__.core.invoke 调用后端命令
fn tauri_invoke(cmd: &str, name: &str) -> Result<JsFuture, String> {
    let window = web_sys::window().ok_or("无法获取 window 对象")?;
    let tauri = js_sys::Reflect::get(&window, &JsValue::from_str("__TAURI__"))
        .map_err(|_| "window.__TAURI__ 不存在（请检查 tauri.conf.json 的 withGlobalTauri）")?;
    let core = js_sys::Reflect::get(&tauri, &JsValue::from_str("core"))
        .map_err(|_| "__TAURI__.core 不存在")?;
    let invoke_fn = js_sys::Reflect::get(&core, &JsValue::from_str("invoke"))
        .map_err(|_| "__TAURI__.core.invoke 不存在")?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| "invoke 不是函数")?;

    let args = js_sys::Object::new();
    js_sys::Reflect::set(&args, &JsValue::from_str("name"), &JsValue::from_str(name))
        .map_err(|_| "构造参数失败")?;

    let ret = invoke_fn
        .call2(&core, &JsValue::from_str(cmd), &args.into())
        .map_err(|_| "invoke 调用抛出异常（检查 capabilities 权限与命令名）")?;
    let promise = ret
        .dyn_into::<js_sys::Promise>()
        .map_err(|_| "invoke 返回值不是 Promise")?;
    Ok(JsFuture::from(promise))
}
