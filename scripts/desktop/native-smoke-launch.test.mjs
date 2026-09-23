import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"

test("the embedded document explains a frontend that never replaces it", () => {
  const document = readFileSync("index.html", "utf8")

  assert.match(document, /data-nessa-load-fallback/)
  assert.match(document, /Loading Nessa/)
  assert.match(document, /If this stays on screen/)
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
