//! WordCraft's headless converter for JavaScript. In the browser (a page or a Web
//! Worker) `main` puts `globalThis.wordcraft` in place:
//!
//! - `convert(name, bytes, to, {markup?, password?})` → `Uint8Array` (throws an `Error`),
//! - `toPdf(name, bytes, opts?)` → `Uint8Array`,
//! - `addFont(name, bytes)` → the families added,
//! - `formats()` → `{open, export}`, and `version`,
//!
//! ready once the module's `init()` promise resolves. Build:
//! `cargo build --release --target wasm32-unknown-unknown -p wordcraft-wasm`, then
//! `wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/wordcraft-wasm.wasm`
//! (`docs/web-embedding.md`). Native builds only print that.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_arch = "wasm32")]
mod js;

/// Runs when the module is initialised: the converter's functions go on `globalThis`.
#[cfg(target_arch = "wasm32")]
fn main() {
    js::install();
}

/// Natively there is nothing to run: point at the wasm build and the CLI.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!(
        "wordcraft-wasm runs in a browser or worker: build it for wasm32-unknown-unknown (see docs/web-embedding.md); natively, use wordcraft-cli convert"
    );
}
