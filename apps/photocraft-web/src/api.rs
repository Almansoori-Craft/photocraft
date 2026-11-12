//! `window.photocraft`: the control channel for the page, as promises.
//!
//! The same JSON methods as the desktop control server (`docs/control-protocol.md`), so an agent
//! driving VectorCraft in a browser tab (or an `<iframe>` in another app) uses the same commands as
//! the CLI and the MCP server:
//!
//! ```js
//! await photocraft.execute("object.rectangle", {x: 0, y: 0, width: 100, height: 50})  // engine command → its result
//! await photocraft.request("ui.inspect")                      // control method → its result
//! await photocraft.commands()                                 // command list
//! const {base64} = await photocraft.screenshot()              // the canvas as PNG (base64)
//! await photocraft.openFile("card.png", bytes)               // opened as a dropped file is
//! photocraft.captureDownloads(true)                           // saves stay in the page…
//! photocraft.files(); await photocraft.readFile("card.png")  // …to be read back
//! // Al Majlis fork (Studio 2c): a photo Smart Object's contents out and back, as PNG bytes.
//! const out = await photocraft.exportSmartObjectBytes(layerId) // {bytes, fingerprint, contentsId, document, …}
//! await photocraft.replaceSmartObjectBytes(layerId, {...out, bytes: edited}) // refused when stale
//! ```
//!
//! Results resolve with the method's `result`; errors reject with an `Error`.

use std::cell::{Cell, RefCell};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use serde_json::{Value, json};
use photocraft_ui_egui::ControlRequest;
use wasm_bindgen::prelude::*;

use crate::saved_files::SavedFiles;


use crate::web::Inbox;

type Waiting = (Receiver<Value>, js_sys::Function, js_sys::Function);

thread_local! {
    static TX: RefCell<Option<Sender<ControlRequest>>> = const { RefCell::new(None) };
    static WAITING: RefCell<Vec<Waiting>> = const { RefCell::new(Vec::new()) };
    static CTX: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
    static INBOX: RefCell<Option<Inbox>> = const { RefCell::new(None) };
    static PLACE_INBOX: RefCell<Option<Inbox>> = const { RefCell::new(None) };
    static CAPTURE: Cell<bool> = const { Cell::new(false) };
    static SAVED: RefCell<SavedFiles> = RefCell::new(SavedFiles::default());
}

/// Wire the page's requests to the app: control requests go to `tx`, opened files to `inbox`.
/// After a graphics restart the app keeps its channel: `tx` is `None` and only the context changes.
pub fn connect(tx: Option<Sender<ControlRequest>>, inbox: Inbox, place_inbox: Inbox, ctx: egui::Context) {
    PLACE_INBOX.with(|i| *i.borrow_mut() = Some(place_inbox));
    if let Some(tx) = tx {
        TX.with(|t| *t.borrow_mut() = Some(tx));
    }
    INBOX.with(|i| *i.borrow_mut() = Some(inbox));
    CTX.with(|c| *c.borrow_mut() = Some(ctx));
}

fn repaint() {
    CTX.with(|c| {
        if let Some(ctx) = c.borrow().as_ref() {
            ctx.request_repaint();
        }
    });
}

/// Keep a save instead of downloading it, when the page asked for that. True when kept.
pub fn capture(path: &str, bytes: &[u8]) -> bool {
    if !CAPTURE.with(Cell::get) {
        return false;
    }
    SAVED.with(|s| s.borrow_mut().keep(path, bytes));
    true
}

fn to_js(v: &Value) -> JsValue {
    js_sys::JSON::parse(&v.to_string()).unwrap_or(JsValue::NULL)
}

fn from_js(v: &JsValue) -> Value {
    if v.is_undefined() || v.is_null() {
        return json!({});
    }
    js_sys::JSON::stringify(v).ok().and_then(|s| s.as_string()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}))
}

/// Settle the promises of answered requests (called every frame).
pub fn poll_replies() {
    WAITING.with(|w| {
        w.borrow_mut().retain(|(rx, resolve, reject)| match rx.try_recv() {
            Ok(v) => {
                if v.get("ok").and_then(Value::as_bool) == Some(true) {
                    let _ = resolve.call1(&JsValue::NULL, &to_js(v.get("result").unwrap_or(&Value::Null)));
                } else {
                    let msg = v.get("error").and_then(Value::as_str).unwrap_or("error").to_string();
                    let _ = reject.call1(&JsValue::NULL, &js_sys::Error::new(&msg));
                }
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => {
                let _ = reject.call1(&JsValue::NULL, &js_sys::Error::new("request dropped"));
                false
            }
        });
    });
}

fn request(method: &str, params: Value) -> js_sys::Promise {
    let method = method.to_string();
    let p = js_sys::Promise::new(&mut |resolve, reject| {
        let (req, rx) = ControlRequest::new(method.clone(), params.clone());
        let sent = TX.with(|t| t.borrow().as_ref().is_some_and(|tx| tx.send(req).is_ok()));
        if sent {
            WAITING.with(|w| w.borrow_mut().push((rx, resolve, reject)));
        } else {
            let _ = reject.call1(&JsValue::NULL, &js_sys::Error::new("Al Majlis Design is not running yet"));
        }
    });
    repaint();
    p
}

fn rejected(msg: &str) -> js_sys::Promise {
    js_sys::Promise::reject(&js_sys::Error::new(msg).into())
}

fn get(obj: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(obj, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

/// A non-negative integer id from JS (a number, as `inspect` reports layer ids).
fn id_of(v: &JsValue) -> Option<u64> {
    v.as_f64().filter(|n| n.is_finite() && *n >= 0.0 && n.fract() == 0.0 && *n <= 9_007_199_254_740_991.0).map(|n| n as u64)
}

/// `exportSmartObjectBytes(layerId)`: the raster Smart Object's own contents as PNG bytes at their
/// native size, with the identity `replaceSmartObjectBytes` checks.
fn export_smart_object_bytes(layer: JsValue) -> js_sys::Promise {
    use base64::Engine as _;
    let Some(layer) = id_of(&layer) else { return rejected("exportSmartObjectBytes: layerId must be a layer id") };
    let reply = request("engine.execute", json!({"command": "layer.smartObjects.exportPng", "params": {"layer": layer}}));
    wasm_bindgen_futures::future_to_promise(async move {
        let r = wasm_bindgen_futures::JsFuture::from(reply).await?;
        let encoded = get(&r, "pngBase64").as_string().ok_or_else(|| js_sys::Error::new("exportSmartObjectBytes: no contents"))?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|e| js_sys::Error::new(&format!("exportSmartObjectBytes: {e}")))?;
        let out = js_sys::Object::new();
        js_sys::Reflect::set(&out, &"bytes".into(), &js_sys::Uint8Array::from(bytes.as_slice()))?;
        for key in ["fingerprint", "contentsId", "document", "layer", "width", "height", "fileName"] {
            js_sys::Reflect::set(&out, &key.into(), &get(&r, key))?;
        }
        Ok(out.into())
    })
}

/// `replaceSmartObjectBytes(layerId, {bytes, fingerprint, contentsId, document})`: write edited PNG
/// bytes back; PhotoCraft refuses when the contents changed since the export (stale identity).
fn replace_smart_object_bytes(layer: JsValue, args: JsValue) -> js_sys::Promise {
    use base64::Engine as _;
    let Some(layer) = id_of(&layer) else { return rejected("replaceSmartObjectBytes: layerId must be a layer id") };
    if !args.is_object() {
        return rejected("replaceSmartObjectBytes: pass {bytes, fingerprint, contentsId, document}");
    }
    let raw = get(&args, "bytes");
    let bytes = if let Some(a) = raw.dyn_ref::<js_sys::Uint8Array>() {
        a.to_vec()
    } else if let Some(b) = raw.dyn_ref::<js_sys::ArrayBuffer>() {
        js_sys::Uint8Array::new(b).to_vec()
    } else {
        return rejected("replaceSmartObjectBytes: bytes must be a Uint8Array or ArrayBuffer of PNG data");
    };
    let (Some(fingerprint), Some(contents), Some(document)) = (get(&args, "fingerprint").as_string(), id_of(&get(&args, "contentsId")), id_of(&get(&args, "document")))
    else {
        return rejected("replaceSmartObjectBytes: fingerprint, contentsId and document come from exportSmartObjectBytes");
    };
    request(
        "engine.execute",
        json!({"command": "layer.smartObjects.updatePng", "params": {
            "layer": layer, "document": document, "contentsId": contents, "fingerprint": fingerprint,
            "pngBase64": base64::engine::general_purpose::STANDARD.encode(&bytes),
        }}),
    )
}

fn set_fn(obj: &js_sys::Object, name: &str, f: JsValue) -> Result<(), JsValue> {
    js_sys::Reflect::set(obj, &name.into(), &f).map(|_| ())
}

/// Install `window.photocraft`.
pub fn install() -> Result<(), JsValue> {
    let obj = js_sys::Object::new();
    set_fn(
        &obj,
        "request",
        Closure::<dyn Fn(String, JsValue) -> js_sys::Promise>::new(|method: String, params: JsValue| request(&method, from_js(&params)))
            .into_js_value(),
    )?;
    set_fn(
        &obj,
        "execute",
        Closure::<dyn Fn(String, JsValue) -> js_sys::Promise>::new(|command: String, params: JsValue| {
            request("engine.execute", json!({"command": command, "params": from_js(&params)}))
        })
        .into_js_value(),
    )?;
    set_fn(&obj, "commands", Closure::<dyn Fn() -> js_sys::Promise>::new(|| request("engine.commands", json!({}))).into_js_value())?;
    set_fn(&obj, "inspect", Closure::<dyn Fn() -> js_sys::Promise>::new(|| request("ui.inspect", json!({}))).into_js_value())?;
    set_fn(
        &obj,
        "screenshot",
        Closure::<dyn Fn(JsValue) -> js_sys::Promise>::new(|params: JsValue| request("ui.screenshot", from_js(&params))).into_js_value(),
    )?;
    set_fn(
        &obj,
        "openFile",
        Closure::<dyn Fn(String, js_sys::Uint8Array) -> JsValue>::new(|name: String, bytes: js_sys::Uint8Array| {
            let queued = INBOX.with(|i| {
                i.borrow().as_ref().is_some_and(|inbox| {
                    inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes.to_vec()));
                    true
                })
            });
            repaint();
            JsValue::from_bool(queued)
        })
        .into_js_value(),
    )?;
    // Al Majlis fork (F5b): place a file into the open document as a dropped-on-canvas file is placed
    // (a Smart Object when the preference says so), since file.placeEmbedded needs a path the web lacks.
    set_fn(
        &obj,
        "placeFile",
        Closure::<dyn Fn(String, js_sys::Uint8Array) -> JsValue>::new(|name: String, bytes: js_sys::Uint8Array| {
            let queued = PLACE_INBOX.with(|i| {
                i.borrow().as_ref().is_some_and(|inbox| {
                    inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes.to_vec()));
                    true
                })
            });
            repaint();
            JsValue::from_bool(queued)
        })
        .into_js_value(),
    )?;
    // Al Majlis fork (Studio 2c): exportContents / replaceContents need a file path the web lacks.
    set_fn(
        &obj,
        "exportSmartObjectBytes",
        Closure::<dyn Fn(JsValue) -> js_sys::Promise>::new(export_smart_object_bytes).into_js_value(),
    )?;
    set_fn(
        &obj,
        "replaceSmartObjectBytes",
        Closure::<dyn Fn(JsValue, JsValue) -> js_sys::Promise>::new(replace_smart_object_bytes).into_js_value(),
    )?;
    set_fn(&obj, "captureDownloads", Closure::<dyn Fn(bool)>::new(|on: bool| CAPTURE.with(|c| c.set(on))).into_js_value())?;
    set_fn(
        &obj,
        "files",
        Closure::<dyn Fn() -> JsValue>::new(|| {
            SAVED.with(|s| to_js(&Value::Array(s.borrow().list().map(|(n, size)| json!({"path": n, "size": size})).collect())))
        })
        .into_js_value(),
    )?;
    set_fn(
        &obj,
        "readFile",
        Closure::<dyn Fn(String) -> Result<JsValue, JsValue>>::new(|name: String| {
            SAVED.with(|s| {
                s.borrow()
                    .get(&name)
                    .map(|b| js_sys::Uint8Array::from(b).into())
                    .ok_or_else(|| js_sys::Error::new(&format!("{name}: not found")).into())
            })
        })
        .into_js_value(),
    )?;
    let w = web_sys::window().ok_or("no window")?;
    js_sys::Reflect::set(&w, &"photocraft".into(), &obj)?;
    Ok(())
}
