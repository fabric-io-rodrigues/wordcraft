//! WordCraft's converter without a window: a document's bytes in, a PDF (or any
//! format WordCraft saves) out, as `wordcraft-cli convert` does with files. `main.rs` puts these
//! functions on JavaScript's `globalThis.wordcraft` (a page or a Web Worker); here they are plain
//! Rust, tested natively. See `docs/web-embedding.md`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub use wordcraft_engine::io::{MAX_CONVERT_BYTES, OPEN_EXTS, SAVE_EXTS};

/// How a conversion reads and writes.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Print Markup: tracked changes marked up and the comments shown.
    pub markup: bool,
    /// The password of a password-protected Word document.
    pub password: Option<String>,
}

/// Open `bytes` (the format comes from `name`'s extension) and write it as `to` (`"pdf"`,
/// `"docx"`…, one of [`SAVE_EXTS`]).
pub fn convert(name: &str, bytes: &[u8], to: &str, opts: &Options) -> Result<Vec<u8>, String> {
    let opts = wordcraft_engine::io::ConvertOptions { markup: opts.markup, password: opts.password.as_deref(), ..Default::default() };
    wordcraft_engine::io::convert_bytes(name, bytes, to, &opts)
}

/// [`convert`] to PDF.
pub fn to_pdf(name: &str, bytes: &[u8], opts: &Options) -> Result<Vec<u8>, String> {
    convert(name, bytes, "pdf", opts)
}

/// Make a font (TTF, OTF, TTC, WOFF, WOFF2) available to later conversions: the browser has no
/// system fonts, so text in a font WordCraft doesn't bundle falls back otherwise. Returns the
/// families it added.
pub fn add_font(name: &str, bytes: &[u8]) -> Result<Vec<String>, String> {
    let installed = wordcraft_fonts::FontDb::global().install_font(name, bytes).map_err(|e| e.to_string())?;
    let mut families: Vec<String> = installed.font.faces.into_iter().map(|(family, _)| family).collect();
    families.dedup();
    Ok(families)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_a_docx_to_pdf() {
        let docx = wordcraft_engine::io::save_bytes("in.docx", &wordcraft_engine::sample::sample_document()).unwrap();
        let pdf = to_pdf("in.docx", &docx, &Options::default()).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
        let marked = to_pdf("in.docx", &docx, &Options { markup: true, password: None }).unwrap();
        assert!(marked.starts_with(b"%PDF-"));
        let html = convert("in.md", b"# Hi\n", "html", &Options::default()).unwrap();
        assert!(String::from_utf8_lossy(&html).contains("Hi"));
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(convert("in.md", b"x", "exe", &Options::default()).is_err());
        assert!(to_pdf("x.docx", b"not a zip", &Options::default()).is_err());
        assert!(add_font("junk.ttf", b"junk").is_err());
        assert!(add_font("empty.woff2", b"").is_err());
    }
}
