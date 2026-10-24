# Embedding WordCraft in a web page

> **Last reviewed:** 2026-10-24 · **Last updated:** 2026-10-24 · **Change:** minor (parent origins listed by the deployment, ordered opens and exports, streamed `?file=` cap; previously major: new page: `?file=`, `window.wordcraft`, `postMessage` and the headless `wordcraft-wasm` converter)

A page can use WordCraft in two ways:

- **The editor** (`apps/wordcraft-web`): the full word processor, on the page or in an `<iframe>`.
  The page hands it a file and gets the document back as bytes (a PDF, a `.docx`…).
- **The converter** (`apps/wordcraft-wasm`): no window and no egui. It turns a document's bytes into
  a PDF or another format, in a page or a Web Worker, the way `wordcraft-cli convert` does with
  files.

Both read and write through `wordcraft_engine::io::convert_bytes` / `save_bytes_opts`, so they make
the same output as the desktop app and the CLI.

## The editor

Build it with `cd apps/wordcraft-web && trunk build --release` (output in `dist/web`), then serve that
folder.

### A file in the address: `?file=`

```
https://example.org/wordcraft/?file=/docs/report.docx
https://example.org/wordcraft/?file=https%3A%2F%2Fcdn.example.org%2Fa%3Fid%3D7&name=Report.docx
```

- `file` is resolved against the page's address and fetched with `fetch()`. Only `http:`, `https:`
  and `blob:` addresses are fetched. A file on another origin needs CORS headers from its server.
- `name` gives the file name, and its extension gives the format, when the URL path doesn't end in
  one. The extension must be a document format the editor opens: `docx docm dotx dotm doc dot odt
  rtf txt md html htm tex json xml`.
- Files over 200 MB are refused: by `Content-Length` before reading when the server sends one,
  and otherwise as soon as the download passes 200 MB (it is read in chunks and cancelled there).
- The file opens before any `window.wordcraft` or message request is answered, so an export asked
  for meanwhile waits for it instead of getting the blank document.
- When the file can't be opened, the reason is shown on the status line and the editor opens a new
  blank document, never an error page.

### From the same page: `window.wordcraft`

Once the editor has started, `window.wordcraft` is there and `window` gets a `wordcraft-ready`
event. Only scripts of the editor's own origin can reach it.

```js
addEventListener("wordcraft-ready", async () => {
  const bytes = new Uint8Array(await (await fetch("report.docx")).arrayBuffer());
  await wordcraft.open("report.docx", bytes);       // resolves once it has opened: {name}
  const pdf = await wordcraft.exportPdf();           // Uint8Array, "%PDF-…", of report.docx
  const docx = await wordcraft.export("docx");       // any format in formats().export
  const marked = await wordcraft.exportPdf({ markup: true }); // Print Markup: comments and changes
});
```

| Member | What it does |
|---|---|
| `open(name, Uint8Array \| ArrayBuffer, {discardChanges?})` | Opens the document. The promise resolves with `{name}` once it **has** opened, and rejects for a name WordCraft doesn't open, a file over 200 MB, a file that isn't the document its name says, or unsaved changes in the open document (unless `discardChanges: true`: the user is never asked, so the outcome is known). |
| `exportPdf({markup?})` | `Promise<Uint8Array>`: the open document as a PDF, made with the same options as File › Export › PDF. `markup` overrides File › Print › Print Markup. |
| `export(format, {markup?})` | The same in any format WordCraft saves: `docx docm dotx dotm pdf txt md html rtf odt tex png json xml` (`png` is the first page). |
| `formats()` | `{open, export}`: the extension lists. |
| `version` | The WordCraft version. |

Opens and exports are answered in the order they were asked, on the editor's next frames: an
export has the document every open before it left, and an open that failed leaves the document
it found. An export rejects when no document is open (File › Home) or when the format isn't one
WordCraft saves.

### From a parent page: `postMessage`

The editor answers messages only from parent pages **the deployment lists** in its `index.html`.
The page that frames the editor chooses the editor's URL, so a URL parameter can't decide whom to
trust; whoever serves the editor does:

```html
<!-- in the deployed index.html (shipped empty: no parent is answered) -->
<meta name="wordcraft-embed-origins" content="https://app.example.com https://staging.example.com">
```

```html
<!-- on https://app.example.com -->
<iframe id="wc" src="https://example.org/wordcraft/"></iframe>
<script>
  const wc = document.getElementById("wc").contentWindow;
  const target = "https://example.org";
  addEventListener("message", (e) => {
    if (e.origin !== target) return;
    if (e.data.type === "wordcraft:ready") {
      wc.postMessage({ type: "wordcraft:open", id: 1, name: "report.docx", data: bytes }, target);
      wc.postMessage({ type: "wordcraft:export", id: 2, format: "pdf" }, target);
    }
    if (e.data.type === "wordcraft:exported") save(new Blob([e.data.data], { type: "application/pdf" }));
    if (e.data.type === "wordcraft:error") console.error(e.data.id, e.data.message);
  });
</script>
```

| Message | Direction | Fields |
|---|---|---|
| `wordcraft:ready` | editor → parent | sent once the editor listens, to the listed origins only |
| `wordcraft:open` | parent → editor | `id?`, `name`, `data` (`Uint8Array` or `ArrayBuffer`), `discardChanges?` |
| `wordcraft:opened` | editor → parent | `id`, `name`: the document has opened |
| `wordcraft:export` | parent → editor | `id` (any value, echoed back), `format` (default `"pdf"`), `markup?` |
| `wordcraft:exported` | editor → parent | `id`, `format`, `data` (`ArrayBuffer`, transferred) |
| `wordcraft:error` | editor → parent | `id`, `message` |

Messages are answered in the order they arrive, as `window.wordcraft` calls are. Who gets answered:

- Nobody, unless `wordcraft-embed-origins` lists at least one origin. Only `http`/`https` origins
  count; `*` and `null` are ignored.
- Only messages whose `source` is the parent window and whose origin is listed are read. Other
  frames, popups and the page itself are ignored, and so is the editor when it isn't in a frame.
- Answers go to the origin of the message they answer, never to `"*"`.
- A parent with an opaque origin (a sandboxed frame without `allow-same-origin`, a page opened from
  a `file:` URL) can't be listed: `postMessage` can't name such an origin, so no answer could reach
  it.
- To keep other sites from framing the editor at all, serve it with
  `Content-Security-Policy: frame-ancestors <the listed origins>`.

## The converter: `wordcraft-wasm`

```sh
cargo build --release --target wasm32-unknown-unknown -p wordcraft-wasm
wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/wordcraft-wasm.wasm
```

`wasm-bindgen` (the CLI) must be the version in `Cargo.lock` (`cargo install wasm-bindgen-cli
--version <it> --locked`). For a Web Worker, `--target no-modules` works too.

```js
import init from "./pkg/wordcraft-wasm.js";
await init();                                   // runs main(): globalThis.wordcraft is set
const pdf = wordcraft.toPdf("report.docx", bytes);              // Uint8Array
const odt = wordcraft.convert("notes.md", bytes, "odt", { markup: false });
wordcraft.addFont("Inter.woff2", fontBytes);    // ["Inter"]: used by later conversions
```

| Function | Returns / throws |
|---|---|
| `convert(name, bytes, to, {markup?, password?})` | `Uint8Array`. Throws an `Error` saying why for an unknown format, a file over 200 MB, a missing or wrong password, or a file that isn't the document its name says. |
| `toPdf(name, bytes, opts?)` | `convert(…, "pdf", opts)` |
| `addFont(name, bytes)` | The families added (TTF, OTF, TTC, WOFF, WOFF2). The browser has no system fonts, so text in a font WordCraft doesn't bundle falls back to a bundled one unless the font is added first. |
| `formats()` | `{open, export}` |
| `version` | The WordCraft version |

Calls are synchronous, so a long conversion belongs in a Web Worker.

## Why no `#[wasm_bindgen]` exports

The workspace forbids `unsafe` code (`unsafe_code = "forbid"`), and the code `#[wasm_bindgen]`
generates contains some. Both builds publish their API by putting `wasm-bindgen` closures on a
global object (`js_sys::Reflect::set`), which needs none.

## Tests

- `crates/engine/src/tests.rs`: `convert_bytes_*` (DOCX → PDF, Markdown → ODT, hostile input).
- `apps/wordcraft-web/src/host_protocol.rs`: URL parameters, allowed addresses, names, sizes,
  formats and the listed origins (`cargo test -p wordcraft-web`).
- `apps/wordcraft-wasm/src/lib.rs`: conversion and font errors (`cargo test -p wordcraft-wasm`).
- `cargo xtask wasm` checks that `wordcraft-wasm` builds for `wasm32-unknown-unknown`.

## Revision history

- 2026-10-24: parent origins come from the deployment (`wordcraft-embed-origins` in `index.html`)
  instead of `?origin=`; opens resolve once the document has opened and are answered in order with
  exports; `?file=` downloads stop at 200 MB.
- 2026-10-24: new page. `?file=`, `window.wordcraft`, `postMessage` and the `wordcraft-wasm`
  converter.
