//! Tauri JS API 互操作：window.__TAURI__.core.invoke / event.listen（withGlobalTauri）。

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

use crate::dto::ServiceInput;

fn tauri_global() -> Result<JsValue, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("无法获取 window"))?;
    js_sys::Reflect::get(&window, &JsValue::from_str("__TAURI__"))
        .map_err(|_| JsValue::from_str("window.__TAURI__ 不存在（withGlobalTauri）"))
}

/// invoke(command, args)
pub async fn invoke(cmd: &str, args: Option<JsValue>) -> Result<JsValue, JsValue> {
    let tauri = tauri_global()?;
    let core = js_sys::Reflect::get(&tauri, &JsValue::from_str("core"))?;
    let f = js_sys::Reflect::get(&core, &JsValue::from_str("invoke"))?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| JsValue::from_str("__TAURI__.core.invoke 不是函数"))?;
    let ret = match args {
        Some(a) => f.call2(&core, &JsValue::from_str(cmd), &a)?,
        None => f.call1(&core, &JsValue::from_str(cmd))?,
    };
    JsFuture::from(ret.dyn_into::<js_sys::Promise>()?).await
}

/// invoke + serde 反序列化返回值
pub async fn invoke_t<T: serde::de::DeserializeOwned>(cmd: &str, args: Option<JsValue>) -> T {
    let raw = invoke(cmd, args)
        .await
        .unwrap_or_else(|e| wasm_bindgen::throw_str(&js_str(&e)));
    serde_wasm_bindgen::from_value(raw).unwrap_or_else(|e| {
        wasm_bindgen::throw_str(&format!("反序列化 {cmd} 返回值失败：{e}"))
    })
}

pub async fn invoke_unit_res(cmd: &str, args: Option<JsValue>) -> Result<(), String> {
    let raw = invoke(cmd, args).await.map_err(|e| js_str(&e))?;
    if raw.is_undefined() || raw.is_null() {
        return Ok(());
    }
    serde_wasm_bindgen::from_value(raw).map_err(|e| format!("解析 {cmd} 返回值失败：{e}"))
}

/// 从 Result<T, Vec<String>> 提取错误列表（后端 upsert/save 校验错误）
pub async fn invoke_errors(cmd: &str, args: Option<JsValue>) -> Result<(), Vec<String>> {
    match invoke(cmd, args).await {
        Ok(_) => Ok(()),
        Err(e) => {
            if let Ok(list) = serde_wasm_bindgen::from_value::<Vec<String>>(e.clone()) {
                Err(list)
            } else if let Some(s) = e.as_string() {
                Err(vec![s])
            } else {
                Err(vec![js_str(&e)])
            }
        }
    }
}

fn js_str(v: &JsValue) -> String {
    v.as_string().unwrap_or_else(|| format!("{v:?}"))
}

/// listen(event, cb) → 返回 unlisten 函数
pub async fn listen<F: Fn(JsValue) + 'static>(
    event: &str,
    cb: F,
) -> Result<js_sys::Function, JsValue> {
    let tauri = tauri_global()?;
    let event_mod = js_sys::Reflect::get(&tauri, &JsValue::from_str("event"))?;
    let f = js_sys::Reflect::get(&event_mod, &JsValue::from_str("listen"))?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| JsValue::from_str("__TAURI__.event.listen 不是函数"))?;
    let wrapped = wasm_bindgen::closure::Closure::wrap(Box::new(move |payload: JsValue| {
        // Tauri 事件回调签名 (event: Event { payload, ... })；payload 在 .payload 上
        let p = js_sys::Reflect::get(&payload, &JsValue::from_str("payload"))
            .unwrap_or(payload.clone());
        cb(p);
    }) as Box<dyn Fn(JsValue)>);
    let ret = f.call2(
        &event_mod,
        &JsValue::from_str(event),
        wrapped.as_ref().unchecked_ref::<js_sys::Function>(),
    )?;
    let unlisten = JsFuture::from(ret.dyn_into::<js_sys::Promise>()?).await?;
    wrapped.forget(); // 常驻监听；unlisten 句柄仍可用
    Ok(unlisten.dyn_into::<js_sys::Function>()?)
}

pub fn to_js<T: serde::Serialize>(v: &T) -> JsValue {
    serde_wasm_bindgen::to_value(v).unwrap_or_else(|e| {
        wasm_bindgen::throw_str(&format!("序列化参数失败：{e}"))
    })
}

pub fn invoke_args_input(input: &ServiceInput) -> JsValue {
    let obj = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("input"), &to_js(input));
    obj.into()
}

pub fn invoke_args_id(id: &str) -> JsValue {
    let obj = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("id"), &JsValue::from_str(id));
    obj.into()
}

pub fn invoke_args_log(id: &str, since: u64, limit: usize) -> JsValue {
    let obj = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("id"), &JsValue::from_str(id));
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("since"), &JsValue::from_f64(since as f64));
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("limit"), &JsValue::from_f64(limit as f64));
    obj.into()
}
