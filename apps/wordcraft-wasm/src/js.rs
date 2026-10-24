//! `globalThis.wordcraft`: the converter's functions as JavaScript sees them. Closures, not
//! `#[wasm_bindgen]` exports, which would need `unsafe` code the workspace forbids.

use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;

/// What a function JavaScript calls returns: a value, or an `Error` it throws.
type JsResult = Result<JsValue, JsValue>;

/// Set `key` on a JS object.
fn set(obj: &js_sys::Object, key: &str, value: &JsValue) {
    let _ = js_sys::Reflect::set(obj, &JsValue::from_str(key), value);
}

/// `key` of a JS value (`undefined` when it isn't an object).
fn get(obj: &JsValue, key: &str) -> JsValue {
    if obj.is_object() { js_sys::Reflect::get(obj, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED) } else { JsValue::UNDEFINED }
}

/// A JS `Error` with `msg`, for JavaScript to catch.
fn error(msg: impl AsRef<str>) -> JsValue {
    js_sys::Error::new(msg.as_ref()).into()
}

/// The file name JavaScript passed: its extension gives the format.
fn name_of(name: &JsValue) -> Result<String, JsValue> {
    name.as_string().ok_or_else(|| error("the file needs a name, such as \"report.docx\": its extension gives the format"))
}

/// The bytes of a `Uint8Array` or an `ArrayBuffer`, refused over the converter's size cap
/// before they are copied.
fn bytes_of(data: &JsValue) -> Result<Vec<u8>, JsValue> {
    let array = if let Some(a) = data.dyn_ref::<js_sys::Uint8Array>() {
        a.clone()
    } else if let Some(b) = data.dyn_ref::<js_sys::ArrayBuffer>() {
        js_sys::Uint8Array::new(b)
    } else {
        return Err(error("the file's data must be a Uint8Array or an ArrayBuffer"));
    };
    if usize::try_from(array.length()).map_or(true, |n| n > wordcraft_wasm::MAX_CONVERT_BYTES) {
        return Err(error(format!("the file is larger than {} MB", wordcraft_wasm::MAX_CONVERT_BYTES >> 20)));
    }
    Ok(array.to_vec())
}

/// `{markup?, password?}` as [`wordcraft_wasm::Options`]; anything else is the defaults.
fn options_of(opts: &JsValue) -> wordcraft_wasm::Options {
    wordcraft_wasm::Options { markup: get(opts, "markup").as_bool().unwrap_or(false), password: get(opts, "password").as_string() }
}

/// A conversion's bytes as a `Uint8Array`, or its error thrown.
fn bytes_out(r: Result<Vec<u8>, String>) -> JsResult {
    r.map(|bytes| js_sys::Uint8Array::from(bytes.as_slice()).into()).map_err(error)
}

/// A JS array of strings.
fn list(items: &[&str]) -> JsValue {
    items.iter().map(|e| JsValue::from_str(e)).collect::<js_sys::Array>().into()
}

/// Put `globalThis.wordcraft` in place: `convert`, `toPdf`, `addFont`, `formats` and `version`.
pub(crate) fn install() {
    let api = js_sys::Object::new();
    let convert =
        Closure::<dyn Fn(JsValue, JsValue, JsValue, JsValue) -> JsResult>::new(|name: JsValue, data: JsValue, to: JsValue, opts: JsValue| {
            let to = to.as_string().ok_or_else(|| error("say which format to convert to, such as \"pdf\""))?;
            bytes_out(wordcraft_wasm::convert(&name_of(&name)?, &bytes_of(&data)?, &to, &options_of(&opts)))
        });
    set(&api, "convert", &convert.into_js_value());
    let to_pdf = Closure::<dyn Fn(JsValue, JsValue, JsValue) -> JsResult>::new(|name: JsValue, data: JsValue, opts: JsValue| {
        bytes_out(wordcraft_wasm::to_pdf(&name_of(&name)?, &bytes_of(&data)?, &options_of(&opts)))
    });
    set(&api, "toPdf", &to_pdf.into_js_value());
    let add_font = Closure::<dyn Fn(JsValue, JsValue) -> JsResult>::new(|name: JsValue, data: JsValue| {
        let families = wordcraft_wasm::add_font(&name_of(&name)?, &bytes_of(&data)?).map_err(error)?;
        Ok(families.iter().map(|f| JsValue::from_str(f)).collect::<js_sys::Array>().into())
    });
    set(&api, "addFont", &add_font.into_js_value());
    let formats = Closure::<dyn Fn() -> JsValue>::new(|| {
        let o = js_sys::Object::new();
        set(&o, "open", &list(wordcraft_wasm::OPEN_EXTS));
        set(&o, "export", &list(wordcraft_wasm::SAVE_EXTS));
        o.into()
    });
    set(&api, "formats", &formats.into_js_value());
    set(&api, "version", &JsValue::from_str(env!("CARGO_PKG_VERSION")));
    // A frozen global object refuses it; then nothing can call the converter, and nothing broke.
    let _ = js_sys::Reflect::set(&js_sys::global(), &JsValue::from_str("wordcraft"), &api);
}
