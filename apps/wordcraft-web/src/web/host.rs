//! The page hosting the app: a file handed in with `?file=<url>`,
//! `window.wordcraft.open` or a `wordcraft:open` message opens as a dropped file does (through the
//! inbox), and the document goes back as bytes (`window.wordcraft.exportPdf`/`export`, or a
//! `wordcraft:export` message). What is accepted is decided in `host_protocol`; see
//! `docs/web-embedding.md`.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::json;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wordcraft_ui_egui::{Inbox, WordApp};

use crate::host_protocol::{self as hp, Kind};

/// Where the bytes of an export go.
enum Reply {
    /// `window.wordcraft.export…`: the promise it returned.
    Promise { resolve: js_sys::Function, reject: js_sys::Function },
    /// A `wordcraft:export` message: back to the window that sent it, at its origin only.
    Message { target: web_sys::Window, origin: String, id: JsValue },
}

impl Reply {
    fn send(self, result: Result<(String, Vec<u8>), String>) {
        match self {
            Reply::Promise { resolve, reject } => {
                let r = match result {
                    Ok((_, bytes)) => resolve.call1(&JsValue::NULL, &js_sys::Uint8Array::from(bytes.as_slice())),
                    Err(e) => reject.call1(&JsValue::NULL, &js_sys::Error::new(&e)),
                };
                if let Err(e) = r {
                    log::warn!("host: couldn't settle the promise: {e:?}");
                }
            }
            Reply::Message { target, origin, id } => {
                let msg = js_sys::Object::new();
                let transfer = js_sys::Array::new();
                set(&msg, "id", &id);
                match result {
                    Ok((format, bytes)) => {
                        let data = js_sys::Uint8Array::from(bytes.as_slice()).buffer();
                        set(&msg, "type", &hp::MSG_EXPORTED.into());
                        set(&msg, "format", &format.into());
                        set(&msg, "data", &data);
                        transfer.push(&data);
                    }
                    Err(e) => {
                        set(&msg, "type", &hp::MSG_ERROR.into());
                        set(&msg, "message", &e.into());
                    }
                }
                if let Err(e) = target.post_message_with_transfer(&msg, &origin, &transfer) {
                    log::warn!("host: couldn't answer {origin}: {e:?}");
                }
            }
        }
    }
}

struct ExportRequest {
    format: String,
    markup: Option<bool>,
    reply: Reply,
}

/// Exports the host asked for, answered from the app's next frame ([`Host::drain`]), after files
/// that arrived before them have opened.
#[derive(Clone, Default)]
pub(crate) struct Host {
    exports: Rc<RefCell<Vec<ExportRequest>>>,
}

impl Host {
    /// Answer the exports asked for since the last frame with the document as it is now.
    pub(crate) fn drain(&self, app: &WordApp) {
        let asked = std::mem::take(&mut *self.exports.borrow_mut());
        for req in asked {
            let result = if app.has_document() {
                let params = req.markup.map_or_else(|| json!({}), |m| json!({"markup": m}));
                let opts = wordcraft_engine::cmd::file::export_options(&app.session, &params);
                let name = format!("{}.{}", app.title_stem(), req.format);
                wordcraft_engine::io::save_bytes_opts(&name, &app.session.doc, &opts).map(|bytes| (req.format, bytes))
            } else {
                Err("no document is open".to_string())
            };
            if let Err(e) = &result {
                log::warn!("host: export failed: {e}");
            }
            req.reply.send(result);
        }
    }

    fn ask(&self, ctx: &egui::Context, format: String, markup: Option<bool>, reply: Reply) {
        self.exports.borrow_mut().push(ExportRequest { format, markup, reply });
        ctx.request_repaint();
    }
}

fn set(obj: &js_sys::Object, key: &str, value: &JsValue) {
    let _ = js_sys::Reflect::set(obj, &JsValue::from_str(key), value);
}

fn get(obj: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(obj, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

/// Queue a file the host handed in for the app to open (or insert, for a picture), as a drop.
fn accept_file(inbox: &Inbox, ctx: &egui::Context, name: &JsValue, data: &JsValue) -> Result<(), String> {
    let name = name.as_string().ok_or("the file needs a name, such as \"report.docx\"")?;
    let name = hp::accepted_name(&name, &[hp::DOC_EXTS, hp::IMAGE_EXTS])?;
    let bytes = if let Some(a) = data.dyn_ref::<js_sys::Uint8Array>() {
        hp::size_ok(f64::from(a.length()))?;
        a.to_vec()
    } else if let Some(b) = data.dyn_ref::<js_sys::ArrayBuffer>() {
        hp::size_ok(f64::from(b.byte_length()))?;
        js_sys::Uint8Array::new(b).to_vec()
    } else {
        return Err("the file's data must be a Uint8Array or an ArrayBuffer".into());
    };
    inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((name, bytes));
    ctx.request_repaint();
    Ok(())
}

/// `?file=<url>` (and `&name=`): fetch the document and open it. Returns whether one was asked
/// for; failures land on the status line through `messages`.
pub(crate) fn open_from_url(search: &str, inbox: Inbox, ctx: egui::Context, messages: Rc<RefCell<Vec<String>>>) -> bool {
    let Some(raw) = hp::query_param(search, "file").filter(|f| !f.trim().is_empty()) else {
        return false;
    };
    let given_name = hp::query_param(search, "name");
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = fetch_into(&raw, given_name.as_deref(), &inbox).await {
            log::error!("?file=: {e}");
            messages.borrow_mut().push(format!("Couldn't open the file in the address: {e}"));
        }
        ctx.request_repaint();
    });
    true
}

async fn fetch_into(raw: &str, given_name: Option<&str>, inbox: &Inbox) -> Result<(), String> {
    let js = |e: JsValue| format!("{e:?}");
    let window = web_sys::window().ok_or("no window")?;
    let base = window.location().href().map_err(js)?;
    let url = web_sys::Url::new_with_base(raw, &base).map_err(|_| format!("{raw:?} is not a URL"))?;
    let href = url.href();
    if !hp::fetchable(&href) {
        return Err(format!("{href:?}: only http, https and blob addresses are opened"));
    }
    let name = hp::accepted_name(&given_name.map_or_else(|| hp::name_from_url(&href), str::to_string), &[hp::DOC_EXTS])?;
    let resp: web_sys::Response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(&href))
        .await
        .map_err(|e| format!("{href} couldn't be fetched: {}", js(e)))?
        .dyn_into()
        .map_err(|_| "not a response".to_string())?;
    if !resp.ok() {
        return Err(format!("{href}: HTTP {}", resp.status()));
    }
    // Refuse a file announced as too big before reading it.
    if let Ok(Some(len)) = resp.headers().get("content-length") {
        hp::size_ok(len.trim().parse::<f64>().unwrap_or(0.0))?;
    }
    let buf = wasm_bindgen_futures::JsFuture::from(resp.array_buffer().map_err(js)?).await.map_err(js)?;
    let buf: js_sys::ArrayBuffer = buf.dyn_into().map_err(|_| "not an ArrayBuffer".to_string())?;
    hp::size_ok(f64::from(buf.byte_length()))?;
    let bytes = js_sys::Uint8Array::new(&buf).to_vec();
    inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((name, bytes));
    Ok(())
}

/// The export a host asked for: its format and Print Markup, or why it can't be made.
fn export_args(format: &JsValue, opts: &JsValue) -> Result<(String, Option<bool>), String> {
    let format = hp::export_format(format.as_string().as_deref())?;
    let markup = if opts.is_object() { get(opts, "markup").as_bool() } else { None };
    Ok((format, markup))
}

/// A promise for an export, rejected straight away when the request is wrong.
fn export_promise(host: &Host, ctx: &egui::Context, format: &JsValue, opts: &JsValue) -> js_sys::Promise {
    match export_args(format, opts) {
        Ok((format, markup)) => {
            js_sys::Promise::new(&mut |resolve, reject| host.ask(ctx, format.clone(), markup, Reply::Promise { resolve, reject }))
        }
        Err(e) => js_sys::Promise::reject(&js_sys::Error::new(&e)),
    }
}

/// `window.wordcraft`: `open(name, bytes)`, `exportPdf(opts?)`, `export(format, opts?)`,
/// `formats()` and `version`; then the `wordcraft-ready` event on `window`, and a
/// `wordcraft:ready` message to the parent page when the app is in a frame.
pub(crate) fn install(host: &Host, inbox: Inbox, ctx: egui::Context) -> Result<(), String> {
    let js = |e: JsValue| format!("{e:?}");
    let window = web_sys::window().ok_or("no window")?;
    let api = js_sys::Object::new();

    let (open_inbox, open_ctx) = (inbox.clone(), ctx.clone());
    let open = Closure::<dyn Fn(JsValue, JsValue) -> js_sys::Promise>::new(move |name: JsValue, data: JsValue| {
        match accept_file(&open_inbox, &open_ctx, &name, &data) {
            Ok(()) => js_sys::Promise::resolve(&JsValue::UNDEFINED),
            Err(e) => js_sys::Promise::reject(&js_sys::Error::new(&e)),
        }
    });
    set(&api, "open", &open.into_js_value());

    let (pdf_host, pdf_ctx) = (host.clone(), ctx.clone());
    let export_pdf = Closure::<dyn Fn(JsValue) -> js_sys::Promise>::new(move |opts: JsValue| {
        export_promise(&pdf_host, &pdf_ctx, &JsValue::from_str("pdf"), &opts)
    });
    set(&api, "exportPdf", &export_pdf.into_js_value());

    let (any_host, any_ctx) = (host.clone(), ctx.clone());
    let export = Closure::<dyn Fn(JsValue, JsValue) -> js_sys::Promise>::new(move |format: JsValue, opts: JsValue| {
        export_promise(&any_host, &any_ctx, &format, &opts)
    });
    set(&api, "export", &export.into_js_value());

    let formats = Closure::<dyn Fn() -> JsValue>::new(|| {
        let list = |exts: &[&str]| exts.iter().map(|e| JsValue::from_str(e)).collect::<js_sys::Array>();
        let o = js_sys::Object::new();
        set(&o, "open", &list(hp::DOC_EXTS));
        set(&o, "pictures", &list(hp::IMAGE_EXTS));
        set(&o, "export", &list(wordcraft_engine::io::SAVE_EXTS));
        o.into()
    });
    set(&api, "formats", &formats.into_js_value());
    set(&api, "version", &env!("CARGO_PKG_VERSION").into());

    js_sys::Reflect::set(&window, &JsValue::from_str("wordcraft"), &api).map_err(js)?;
    listen_for_messages(&window, host.clone(), inbox, ctx)?;
    let ready = web_sys::CustomEvent::new(hp::READY_EVENT).map_err(js)?;
    window.dispatch_event(&ready).map_err(js)?;
    Ok(())
}

/// The parent page when the app runs in a frame (`None` at the top level).
fn parent_of(window: &web_sys::Window) -> Option<web_sys::Window> {
    window.parent().ok().flatten().filter(|p| !js_sys::Object::is(p, window))
}

/// `postMessage` from the parent page: only messages whose source is the parent
/// window, from the origin `?origin=` names when it names one, are answered, and answers go back
/// to that origin alone.
fn listen_for_messages(window: &web_sys::Window, host: Host, inbox: Inbox, ctx: egui::Context) -> Result<(), String> {
    let Some(parent) = parent_of(window) else { return Ok(()) };
    let expected = hp::query_param(&window.location().search().unwrap_or_default(), "origin");
    let reply_to = parent.clone();
    let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
        let from_parent = e.source().is_some_and(|s| js_sys::Object::is(&s, &reply_to));
        let origin = e.origin();
        if !from_parent || !hp::origin_allowed(expected.as_deref(), &origin) {
            return;
        }
        let data = e.data();
        let Some(kind) = get(&data, "type").as_string().as_deref().and_then(Kind::parse) else { return };
        let id = get(&data, "id");
        let reply = Reply::Message { target: reply_to.clone(), origin, id };
        match kind {
            Kind::Open => {
                if let Err(err) = accept_file(&inbox, &ctx, &get(&data, "name"), &get(&data, "data")) {
                    reply.send(Err(err));
                }
            }
            Kind::Export => match export_args(&get(&data, "format"), &data) {
                Ok((format, markup)) => host.ask(&ctx, format, markup, reply),
                Err(err) => reply.send(Err(err)),
            },
        }
    });
    window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref()).map_err(|e| format!("{e:?}"))?;
    // The listener lives as long as the page.
    on_message.forget();
    // Tell the parent the app is listening; the message carries nothing, so any origin may see it.
    let ready = js_sys::Object::new();
    set(&ready, "type", &hp::MSG_READY.into());
    let target =
        hp::query_param(&window.location().search().unwrap_or_default(), "origin").filter(|o| !o.trim().is_empty()).unwrap_or_else(|| "*".into());
    parent.post_message(&ready, &target).map_err(|e| format!("{e:?}"))?;
    Ok(())
}
