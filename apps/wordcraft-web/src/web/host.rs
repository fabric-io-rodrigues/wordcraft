//! The page hosting the app: a document handed in with `?file=<url>`, `window.wordcraft.open` or
//! a `wordcraft:open` message opens, and the document goes back as bytes
//! (`window.wordcraft.exportPdf`/`export`, or a `wordcraft:export` message). Opens and exports
//! are answered one after another, in the order they were asked, so an export always has the
//! document every open before it left. What is accepted is decided in `host_protocol`; see
//! `docs/web-embedding.md`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use serde_json::json;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wordcraft_ui_egui::WordApp;

use crate::host_protocol::{self as hp, Kind};

/// What a request came to.
enum Done {
    /// The document opened; its name.
    Opened(String),
    /// The document in a format: the format and its bytes.
    Exported(String, Vec<u8>),
}

/// Where the answer to a request goes.
enum Reply {
    /// `window.wordcraft.…`: the promise it returned.
    Promise { resolve: js_sys::Function, reject: js_sys::Function },
    /// A message: back to the window that sent it, at its origin only.
    Message { target: web_sys::Window, origin: String, id: JsValue },
    /// `?file=`: a failure goes to the status line.
    Status(Rc<RefCell<Vec<String>>>),
}

impl Reply {
    /// Settle the promise, post the answer, or show the failure.
    fn send(self, result: Result<Done, String>) {
        if let Err(e) = &result {
            log::warn!("host: {e}");
        }
        match self {
            Reply::Promise { resolve, reject } => {
                let r = match result {
                    Ok(Done::Opened(name)) => {
                        let o = js_sys::Object::new();
                        set(&o, "name", &name.into());
                        resolve.call1(&JsValue::NULL, &o)
                    }
                    Ok(Done::Exported(_, bytes)) => resolve.call1(&JsValue::NULL, &js_sys::Uint8Array::from(bytes.as_slice())),
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
                    Ok(Done::Opened(name)) => {
                        set(&msg, "type", &hp::MSG_OPENED.into());
                        set(&msg, "name", &name.into());
                    }
                    Ok(Done::Exported(format, bytes)) => {
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
            Reply::Status(messages) => {
                if let Err(e) = result {
                    messages.borrow_mut().push(format!("Couldn't open the file in the address: {e}"));
                }
            }
        }
    }
}

/// A request from the hosting page, answered in order by [`Host::drain`].
enum Request {
    Open { name: String, bytes: Vec<u8>, discard: bool, reply: Reply },
    Export { format: String, markup: Option<bool>, reply: Reply },
}

/// The hosting page's requests, answered from the app's frames in the order they came.
#[derive(Clone, Default)]
pub(crate) struct Host {
    queue: Rc<RefCell<VecDeque<Request>>>,
    /// The `?file=` document is still being fetched: requests wait for it, so an export made
    /// meanwhile doesn't get the blank document it replaces.
    fetching: Rc<Cell<bool>>,
}

impl Host {
    /// Answer the requests made since the last frame, in order, once no `?file=` fetch is pending.
    pub(crate) fn drain(&self, app: &mut WordApp) {
        if self.fetching.get() {
            return;
        }
        loop {
            // Each request is taken alone: answering one runs page code that may queue another.
            let Some(req) = self.queue.borrow_mut().pop_front() else { break };
            match req {
                Request::Open { name, bytes, discard, reply } => reply.send(open(app, &name, &bytes, discard).map(Done::Opened)),
                Request::Export { format, markup, reply } => reply.send(export(app, &format, markup).map(|b| Done::Exported(format, b))),
            }
        }
    }

    /// Queue a request and wake the app to answer it.
    fn ask(&self, ctx: &egui::Context, req: Request) {
        self.queue.borrow_mut().push_back(req);
        ctx.request_repaint();
    }
}

/// Open a document the host handed in. Unsaved changes are never dropped unless the host says
/// so (`discardChanges`): the request fails instead of asking the user, so the host knows which
/// document it has.
fn open(app: &mut WordApp, name: &str, bytes: &[u8], discard: bool) -> Result<String, String> {
    if app.has_document() && app.session.dirty && !discard {
        return Err("the open document has unsaved changes: export or save it first, or open with {discardChanges: true}".into());
    }
    let data = wordcraft_engine::cmd::insert::base64_encode(bytes);
    app.execute("file.open", json!({"path": name, "data": data}))?;
    app.ui.backstage = false;
    Ok(name.to_string())
}

/// The open document as `format`, as File › Export writes it.
fn export(app: &WordApp, format: &str, markup: Option<bool>) -> Result<Vec<u8>, String> {
    if !app.has_document() {
        return Err("no document is open".into());
    }
    let params = markup.map_or_else(|| json!({}), |m| json!({"markup": m}));
    let opts = wordcraft_engine::cmd::file::export_options(&app.session, &params);
    wordcraft_engine::io::save_bytes_opts(&format!("{}.{format}", app.title_stem()), &app.session.doc, &opts)
}

/// Set `key` on a JS object.
fn set(obj: &js_sys::Object, key: &str, value: &JsValue) {
    let _ = js_sys::Reflect::set(obj, &JsValue::from_str(key), value);
}

/// `key` of a JS value (`undefined` when it has none).
fn get(obj: &JsValue, key: &str) -> JsValue {
    if obj.is_object() { js_sys::Reflect::get(obj, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED) } else { JsValue::UNDEFINED }
}

/// An open request from what the host handed in: a document name and its bytes, refused over the
/// size cap before they are copied.
fn open_request(name: &JsValue, data: &JsValue, opts: &JsValue, reply: Reply) -> Result<Request, (String, Reply)> {
    let checked = (|| {
        let name = name.as_string().ok_or("the file needs a name, such as \"report.docx\"")?;
        let name = hp::accepted_name(&name, &[hp::DOC_EXTS])?;
        let bytes = if let Some(a) = data.dyn_ref::<js_sys::Uint8Array>() {
            hp::size_ok(f64::from(a.length()))?;
            a.to_vec()
        } else if let Some(b) = data.dyn_ref::<js_sys::ArrayBuffer>() {
            hp::size_ok(f64::from(b.byte_length()))?;
            js_sys::Uint8Array::new(b).to_vec()
        } else {
            return Err("the file's data must be a Uint8Array or an ArrayBuffer".to_string());
        };
        Ok((name, bytes))
    })();
    match checked {
        Ok((name, bytes)) => Ok(Request::Open { name, bytes, discard: get(opts, "discardChanges").as_bool().unwrap_or(false), reply }),
        Err(e) => Err((e, reply)),
    }
}

/// An export request: its format and Print Markup, or why it can't be made.
fn export_request(format: &JsValue, opts: &JsValue, reply: Reply) -> Result<Request, (String, Reply)> {
    match hp::export_format(format.as_string().as_deref()) {
        Ok(format) => Ok(Request::Export { format, markup: get(opts, "markup").as_bool(), reply }),
        Err(e) => Err((e, reply)),
    }
}

/// Queue a request, or answer at once why it can't be made.
fn submit(host: &Host, ctx: &egui::Context, req: Result<Request, (String, Reply)>) {
    match req {
        Ok(req) => host.ask(ctx, req),
        Err((e, reply)) => reply.send(Err(e)),
    }
}

/// A promise that settles when `make`'s request has been answered.
fn promise(host: &Host, ctx: &egui::Context, make: impl FnOnce(Reply) -> Result<Request, (String, Reply)>) -> js_sys::Promise {
    let mut make = Some(make);
    js_sys::Promise::new(&mut |resolve, reject| {
        if let Some(make) = make.take() {
            submit(host, ctx, make(Reply::Promise { resolve, reject }));
        }
    })
}

/// `?file=<url>` (and `&name=`): fetch the document and open it before any other request. Returns
/// whether one was asked for; failures land on the status line through `messages`.
pub(crate) fn open_from_url(search: &str, host: &Host, ctx: egui::Context, messages: Rc<RefCell<Vec<String>>>) -> bool {
    let Some(raw) = hp::query_param(search, "file").filter(|f| !f.trim().is_empty()) else {
        return false;
    };
    let given_name = hp::query_param(search, "name");
    let host = host.clone();
    host.fetching.set(true);
    wasm_bindgen_futures::spawn_local(async move {
        let reply = Reply::Status(messages);
        match fetch(&raw, given_name.as_deref()).await {
            Ok((name, bytes)) => host.queue.borrow_mut().push_front(Request::Open { name, bytes, discard: false, reply }),
            Err(e) => reply.send(Err(e)),
        }
        host.fetching.set(false);
        ctx.request_repaint();
    });
    true
}

/// Fetch `raw` (resolved against the page) as a document: its name and bytes. The body is read in
/// chunks and the download cancelled once it passes the size cap, whatever `Content-Length` said.
async fn fetch(raw: &str, given_name: Option<&str>) -> Result<(String, Vec<u8>), String> {
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
    // A file announced as too big is refused before any of it is read.
    if let Ok(Some(len)) = resp.headers().get("content-length") {
        hp::size_ok(len.trim().parse::<f64>().unwrap_or(0.0))?;
    }
    let body = resp.body().ok_or("the response has no body")?;
    let reader: web_sys::ReadableStreamDefaultReader = body.get_reader().dyn_into().map_err(|_| "not a stream reader".to_string())?;
    let mut bytes = Vec::new();
    loop {
        let chunk = wasm_bindgen_futures::JsFuture::from(reader.read()).await.map_err(js)?;
        if get(&chunk, "done").as_bool().unwrap_or(true) {
            break;
        }
        let part: js_sys::Uint8Array = get(&chunk, "value").dyn_into().map_err(|_| "not bytes".to_string())?;
        let total = bytes.len().saturating_add(usize::try_from(part.length()).unwrap_or(usize::MAX));
        if let Err(e) = hp::size_ok(total as f64) {
            let _ = reader.cancel();
            return Err(e);
        }
        bytes.extend(part.to_vec());
    }
    Ok((name, bytes))
}

/// `window.wordcraft`: `open(name, bytes, opts?)`, `exportPdf(opts?)`, `export(format, opts?)`,
/// `formats()` and `version`; then the `wordcraft-ready` event on `window`, and messages from the
/// parent origins the deployment lists.
pub(crate) fn install(host: &Host, ctx: egui::Context) -> Result<(), String> {
    let js = |e: JsValue| format!("{e:?}");
    let window = web_sys::window().ok_or("no window")?;
    let api = js_sys::Object::new();

    let (open_host, open_ctx) = (host.clone(), ctx.clone());
    let open = Closure::<dyn Fn(JsValue, JsValue, JsValue) -> js_sys::Promise>::new(move |name: JsValue, data: JsValue, opts: JsValue| {
        promise(&open_host, &open_ctx, |reply| open_request(&name, &data, &opts, reply))
    });
    set(&api, "open", &open.into_js_value());

    let (pdf_host, pdf_ctx) = (host.clone(), ctx.clone());
    let export_pdf = Closure::<dyn Fn(JsValue) -> js_sys::Promise>::new(move |opts: JsValue| {
        promise(&pdf_host, &pdf_ctx, |reply| export_request(&JsValue::from_str("pdf"), &opts, reply))
    });
    set(&api, "exportPdf", &export_pdf.into_js_value());

    let (any_host, any_ctx) = (host.clone(), ctx.clone());
    let export = Closure::<dyn Fn(JsValue, JsValue) -> js_sys::Promise>::new(move |format: JsValue, opts: JsValue| {
        promise(&any_host, &any_ctx, |reply| export_request(&format, &opts, reply))
    });
    set(&api, "export", &export.into_js_value());

    let formats = Closure::<dyn Fn() -> JsValue>::new(|| {
        let list = |exts: &[&str]| exts.iter().map(|e| JsValue::from_str(e)).collect::<js_sys::Array>();
        let o = js_sys::Object::new();
        set(&o, "open", &list(hp::DOC_EXTS));
        set(&o, "export", &list(wordcraft_engine::io::SAVE_EXTS));
        o.into()
    });
    set(&api, "formats", &formats.into_js_value());
    set(&api, "version", &env!("CARGO_PKG_VERSION").into());

    js_sys::Reflect::set(&window, &JsValue::from_str("wordcraft"), &api).map_err(js)?;
    listen_for_messages(&window, host.clone(), ctx)?;
    let ready = web_sys::CustomEvent::new(hp::READY_EVENT).map_err(js)?;
    window.dispatch_event(&ready).map_err(js)?;
    Ok(())
}

/// The parent page when the app runs in a frame (`None` at the top level).
fn parent_of(window: &web_sys::Window) -> Option<web_sys::Window> {
    window.parent().ok().flatten().filter(|p| !js_sys::Object::is(p, window))
}

/// The parent origins the deployment lists in `index.html` (`<meta name="wordcraft-embed-origins">`).
fn listed_origins(window: &web_sys::Window) -> Vec<String> {
    let selector = format!("meta[name=\"{}\"]", hp::EMBED_ORIGINS_META);
    let content = window.document().and_then(|d| d.query_selector(&selector).ok().flatten()).and_then(|m| m.get_attribute("content"));
    hp::embed_origins(&content.unwrap_or_default())
}

/// `postMessage` from the parent page: only when the deployment lists parent origins, only from
/// the parent window, and only from a listed origin; answers go back to that origin alone.
fn listen_for_messages(window: &web_sys::Window, host: Host, ctx: egui::Context) -> Result<(), String> {
    let Some(parent) = parent_of(window) else { return Ok(()) };
    let listed = listed_origins(window);
    if listed.is_empty() {
        log::info!("host: no parent origins listed in index.html ({}); messages are not answered", hp::EMBED_ORIGINS_META);
        return Ok(());
    }
    let reply_to = parent.clone();
    let allowed = listed.clone();
    let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
        let from_parent = e.source().is_some_and(|s| js_sys::Object::is(&s, &reply_to));
        let origin = e.origin();
        if !from_parent || !hp::origin_listed(&allowed, &origin) {
            return;
        }
        let data = e.data();
        let Some(kind) = get(&data, "type").as_string().as_deref().and_then(Kind::parse) else { return };
        let reply = Reply::Message { target: reply_to.clone(), origin, id: get(&data, "id") };
        let req = match kind {
            Kind::Open => open_request(&get(&data, "name"), &get(&data, "data"), &data, reply),
            Kind::Export => export_request(&get(&data, "format"), &data, reply),
        };
        submit(&host, &ctx, req);
    });
    window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref()).map_err(|e| format!("{e:?}"))?;
    // The listener lives as long as the page.
    on_message.forget();
    // Tell the parent the app is listening, at the listed origins only.
    let ready = js_sys::Object::new();
    set(&ready, "type", &hp::MSG_READY.into());
    for origin in &listed {
        if let Err(e) = parent.post_message(&ready, origin) {
            log::warn!("host: couldn't tell {origin} the app is ready: {e:?}");
        }
    }
    Ok(())
}
