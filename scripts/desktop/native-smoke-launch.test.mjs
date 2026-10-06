import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"
import { embedLoadFallback } from "../../src/host/load-fallback.mjs"

test("the embedded document shows a loading fallback until the frontend replaces it", () => {
  const document = embedLoadFallback(readFileSync("index.html", "utf8"))

  assert.match(document, /data-nessa-load-fallback/)
  assert.match(document, /data-nessa-load-title>Loading</)
  assert.doesNotMatch(
    document,
    /data-nessa-load-mark|data-nessa-startup-mark|nessa-avatar/,
  )
})

test("only an embedded debug host automatically reveals the completed panel", () => {
  const source = readFileSync("src-tauri/src/main.rs", "utf8")
  assert.match(
    source,
    /#\[cfg\(all\(debug_assertions, feature = "custom-protocol"\)\)\]\s*if let Some\(window\) = app\.get_webview_window\(panel::MAIN_WINDOW\) \{[\s\S]{0,400}?panel::show\(&window, &settings\)/,
  )
  assert.doesNotMatch(
    source,
    /#\[cfg\(debug_assertions\)\]\s*if let Some\(window\) = app\.get_webview_window\(panel::MAIN_WINDOW\)/,
  )
})
