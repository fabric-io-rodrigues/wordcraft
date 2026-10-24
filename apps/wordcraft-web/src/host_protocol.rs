//! The page hosting the browser app: which files it may hand in (`?file=`,
//! `window.wordcraft.open`, `postMessage`), which formats it may ask back, and which parent pages
//! are answered (the origins the deployment lists in `index.html`, never ones the URL names). Everything here is plain Rust, shared with native tests; the browser glue is
//! `web/host.rs`.

/// Documents the browser app opens (the Open picker's filter).
pub(crate) const DOC_EXTS: &[&str] = &["docx", "docm", "dotx", "dotm", "doc", "dot", "odt", "rtf", "txt", "md", "html", "htm", "tex", "json", "xml"];
/// Pictures the browser app inserts (Insert › Pictures' filter).
pub(crate) const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "wmf", "emf"];

/// Largest file a host hands in, as `wordcraft_engine::io::convert_bytes` reads.
pub(crate) const MAX_HOST_FILE_BYTES: usize = wordcraft_engine::io::MAX_CONVERT_BYTES;
/// Longest file name kept from a host (characters).
const MAX_NAME_CHARS: usize = 255;

/// The `postMessage` types the app answers, and the ones it sends.
pub(crate) const MSG_OPEN: &str = "wordcraft:open";
pub(crate) const MSG_EXPORT: &str = "wordcraft:export";
pub(crate) const MSG_READY: &str = "wordcraft:ready";
pub(crate) const MSG_OPENED: &str = "wordcraft:opened";
pub(crate) const MSG_EXPORTED: &str = "wordcraft:exported";
pub(crate) const MSG_ERROR: &str = "wordcraft:error";
/// The event `window` gets once `window.wordcraft` is there.
pub(crate) const READY_EVENT: &str = "wordcraft-ready";
/// The `<meta name=…>` in the deployed `index.html` whose `content` lists the parent origins whose
/// messages are answered. Without it (or with it empty) the app answers no messages: the page that
/// frames the app picks its URL, so only the deployment can say whom to trust.
pub(crate) const EMBED_ORIGINS_META: &str = "wordcraft-embed-origins";

/// What a host message asks for, by its `type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Open,
    Export,
}

impl Kind {
    /// The request a message's `type` names; our own replies and anything else name none.
    pub(crate) fn parse(ty: &str) -> Option<Kind> {
        match ty {
            MSG_OPEN => Some(Kind::Open),
            MSG_EXPORT => Some(Kind::Export),
            _ => None,
        }
    }
}

/// The value of `key` in a URL query (`?a=1&file=x.docx`, with or without the `?`),
/// percent-decoded; `+` is a space. The first one wins.
pub(crate) fn query_param(search: &str, key: &str) -> Option<String> {
    search.trim_start_matches('?').split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (percent_decode(k) == key).then(|| percent_decode(v))
    })
}

/// `%XX` escapes decoded (an invalid one stays as it is) and `+` read as a space; bytes that
/// aren't UTF-8 become U+FFFD.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        // Two hex digits exactly: `from_str_radix` alone would take `%+A` (a sign) as 0x0A.
        let hex = b
            .get(i + 1..i + 3)
            .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (c, hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether the app fetches `href` (already resolved against the page) for `?file=`: web URLs and
/// the page's own `blob:` URLs only — never `javascript:`, `data:` or `file:`.
pub(crate) fn fetchable(href: &str) -> bool {
    let scheme = href.split_once(':').map(|(s, _)| s.to_ascii_lowercase());
    matches!(scheme.as_deref(), Some("http" | "https" | "blob"))
}

/// The file name a URL names: the last segment of its path, percent-decoded ("document" when
/// the path ends in `/`).
pub(crate) fn name_from_url(href: &str) -> String {
    let path = href.split(['?', '#']).next().unwrap_or(href);
    let last = path.rsplit('/').next().unwrap_or(path);
    let name = percent_decode(last);
    if name.trim().is_empty() { "document".into() } else { name }
}

/// A host's file name made safe to use, if its extension is one of `exts`: only the last path
/// component (no folders, no `..`), no control characters, at most 255 characters.
pub(crate) fn accepted_name(name: &str, exts: &[&[&str]]) -> Result<String, String> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let clean: String = last.chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect();
    let clean = clean.trim();
    let ext = clean.rsplit_once('.').map(|(stem, e)| (stem, e.to_ascii_lowercase()));
    match ext {
        Some((stem, e)) if !stem.is_empty() && exts.iter().any(|list| list.contains(&e.as_str())) => Ok(clean.to_string()),
        _ => Err(format!("{clean:?}: not a document WordCraft opens")),
    }
}

/// Whether a file of `len` bytes may come in from the host.
pub(crate) fn size_ok(len: f64) -> Result<(), String> {
    if len.is_finite() && len >= 0.0 && len <= MAX_HOST_FILE_BYTES as f64 {
        Ok(())
    } else {
        Err(format!("the file is larger than {} MB", MAX_HOST_FILE_BYTES >> 20))
    }
}

/// The format a host asks the document back in: an extension WordCraft saves, lower-case and
/// without its dot ("pdf" when none is given).
pub(crate) fn export_format(asked: Option<&str>) -> Result<String, String> {
    let f = asked.unwrap_or("pdf").trim().trim_start_matches('.').to_ascii_lowercase();
    if wordcraft_engine::io::can_convert_to(&f) {
        Ok(f)
    } else {
        Err(format!("can't export as {f:?}: the formats are {}", wordcraft_engine::io::SAVE_EXTS.join(", ")))
    }
}

/// The origins a deployment lists in its `wordcraft-embed-origins` meta tag (separated by spaces
/// or commas): `http`/`https` origins only, without a path and with any trailing `/` dropped.
/// `*`, `null` (an opaque origin, which no reply can reach) and anything else are left out.
pub(crate) fn embed_origins(content: &str) -> Vec<String> {
    content
        .split([' ', ',', '\n', '\t'])
        .map(|o| o.trim().trim_end_matches('/'))
        .filter(|o| {
            let Some((scheme, rest)) = o.split_once("://") else { return false };
            matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") && !rest.is_empty() && !rest.contains(['/', '?', '#', '@', '*'])
        })
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Whether a message from `origin` is answered: only when the deployment lists it.
pub(crate) fn origin_listed(listed: &[String], origin: &str) -> bool {
    listed.iter().any(|o| o.eq_ignore_ascii_case(origin))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENS: &[&[&str]] = &[DOC_EXTS, IMAGE_EXTS];

    #[test]
    fn query_params_decode() {
        let q = "?sample&file=https%3A%2F%2Fh.example%2Fa%20b.docx&name=Relat%C3%B3rio+final.docx&file=second";
        assert_eq!(query_param(q, "file").as_deref(), Some("https://h.example/a b.docx"));
        assert_eq!(query_param(q, "name").as_deref(), Some("Relatório final.docx"));
        assert_eq!(query_param(q, "sample").as_deref(), Some(""));
        assert_eq!(query_param(q, "origin"), None);
        assert_eq!(query_param("file=a%2", "file").as_deref(), Some("a%2"), "a cut escape stays");
        assert_eq!(query_param("file=%ZZ%", "file").as_deref(), Some("%ZZ%"));
        assert_eq!(query_param("file=%FF", "file").as_deref(), Some("\u{FFFD}"));
        assert_eq!(query_param("file=%+A", "file").as_deref(), Some("% A"), "a sign is not a hex digit");
        assert_eq!(query_param("file=%-1%2", "file").as_deref(), Some("%-1%2"));
        assert_eq!(query_param("", "file"), None);
    }

    #[test]
    fn only_web_and_blob_urls_are_fetched() {
        for ok in ["https://h.example/a.docx", "HTTP://h/a.docx", "blob:https://h.example/uuid"] {
            assert!(fetchable(ok), "{ok}");
        }
        for bad in ["javascript:alert(1)", "data:text/plain,hi", "file:///etc/passwd", "a.docx", "", "ftp://h/a"] {
            assert!(!fetchable(bad), "{bad}");
        }
    }

    #[test]
    fn url_names_the_file() {
        assert_eq!(name_from_url("https://h/docs/Relat%C3%B3rio.docx?v=2#x"), "Relatório.docx");
        assert_eq!(name_from_url("https://h/docs/"), "document");
        assert_eq!(name_from_url("blob:https://h/1b2c"), "1b2c");
    }

    #[test]
    fn names_are_cleaned_and_checked() {
        assert_eq!(accepted_name("Report.DOCX", OPENS).as_deref(), Ok("Report.DOCX"));
        assert_eq!(accepted_name("../../etc/a.md", OPENS).as_deref(), Ok("a.md"));
        assert_eq!(accepted_name("C:\\x\\photo.png", OPENS).as_deref(), Ok("photo.png"));
        assert_eq!(accepted_name("a\u{0}b\n.txt", OPENS).as_deref(), Ok("ab.txt"));
        for bad in ["", "noext", ".docx", "x.exe", "x.docx/", "..", "x.pdf"] {
            assert!(accepted_name(bad, OPENS).is_err(), "{bad:?}");
        }
        assert!(accepted_name("x.png", &[DOC_EXTS]).is_err(), "?file= opens documents only");
        let long = format!("{}.docx", "a".repeat(10_000));
        assert!(accepted_name(&long, OPENS).is_err(), "cut to 255 characters, the extension goes");
    }

    #[test]
    fn sizes_are_capped() {
        assert!(size_ok(0.0).is_ok());
        assert!(size_ok(MAX_HOST_FILE_BYTES as f64).is_ok());
        for bad in [MAX_HOST_FILE_BYTES as f64 + 1.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(size_ok(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn export_formats() {
        assert_eq!(export_format(None).as_deref(), Ok("pdf"));
        assert_eq!(export_format(Some(" .DOCX ")).as_deref(), Ok("docx"));
        for bad in ["", "exe", "pdf?", "../pdf"] {
            assert!(export_format(Some(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn deployments_list_the_origins() {
        let listed = embed_origins(" https://App.example/ ,http://localhost:8080\nhttps://b.example ");
        assert_eq!(listed, ["https://app.example", "http://localhost:8080", "https://b.example"]);
        for bad in ["*", "null", "https://*.example", "https://h.example/path", "ftp://h", "javascript:x", "https://", "h.example", "https://u@h"] {
            assert!(embed_origins(bad).is_empty(), "{bad:?}");
        }
        assert!(embed_origins("").is_empty(), "no meta content: no messages answered");
        assert!(origin_listed(&listed, "https://app.example"));
        assert!(origin_listed(&listed, "HTTPS://APP.EXAMPLE"));
        assert!(!origin_listed(&listed, "https://app.example.evil"));
        assert!(!origin_listed(&listed, "null"));
        assert!(!origin_listed(&[], "https://app.example"));
        // The shipped page carries the tag, empty: a deployment fills it in.
        let page = include_str!("../index.html");
        assert!(page.contains(&format!("<meta name=\"{EMBED_ORIGINS_META}\" content=\"\">")));
    }

    #[test]
    fn message_kinds() {
        assert_eq!(Kind::parse(MSG_OPEN), Some(Kind::Open));
        assert_eq!(Kind::parse(MSG_EXPORT), Some(Kind::Export));
        for reply in [MSG_READY, MSG_OPENED, MSG_EXPORTED, MSG_ERROR, READY_EVENT] {
            assert_eq!(Kind::parse(reply), None, "{reply}: our own replies are not requests");
        }
        assert_eq!(Kind::parse("open"), None);
    }
}
