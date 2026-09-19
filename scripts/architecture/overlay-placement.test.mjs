import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"
import { overlayPlacementViolations } from "./overlay-placement.mjs"

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..")

const builder = (calls) => `
/// Doc comments may say .center() — this reads code, not prose.
fn build_setup_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, SETUP_WINDOW, WebviewUrl::App("x".into()))
        .inner_size(SETUP_WIDTH, SETUP_HEIGHT)${calls}
        .visible(false)
        .build()
}

fn elsewhere(window: &WebviewWindow) {
    let _ = window.center();
}
`

test("a setup window builder that asks for no position passes", () => {
  assert.deepEqual(overlayPlacementViolations("src-tauri/src/panel.rs", builder("")), [])
})

test("a builder position is rejected however it is spelled", () => {
  for (const call of ["\n        .center()", "\n        .position(10.0, 20.0)"]) {
    assert.equal(
      overlayPlacementViolations("src-tauri/src/panel.rs", builder(call)).length,
      1,
    )
  }
})

test("the rule only reads the file that owns the builder", () => {
  assert.deepEqual(
    overlayPlacementViolations("src-tauri/src/platform/mod.rs", builder(".center()")),
    [],
  )
})

test("a builder that moves away takes its check with it", () => {
  assert.equal(
    overlayPlacementViolations("src-tauri/src/panel.rs", "fn other() {}").length,
    1,
  )
})

test("the shipped setup window asks the builder for no position", () => {
  const path = join(root, "src-tauri", "src", "panel.rs")
  assert.deepEqual(
    overlayPlacementViolations("src-tauri/src/panel.rs", readFileSync(path, "utf8")),
    [],
  )
})
