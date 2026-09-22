import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"

test("developer console forwarding has one command permission for both bundled windows", () => {
  const capability = JSON.parse(
    readFileSync("src-tauri/capabilities/dev-console.json", "utf8"),
  )
  assert.deepEqual(capability.windows, ["main", "setup"])
  assert.deepEqual(capability.permissions, ["dev-console:allow-forward-webview-console"])

  const build = readFileSync("src-tauri/build.rs", "utf8")
  assert.match(build, /plugin\(\s*"dev-console"/)
  assert.match(build, /commands\(&\["forward_webview_console"\]\)/)

  const host = readFileSync("src-tauri/src/diagnostics.rs", "utf8")
  assert.match(host, /#\[cfg\(debug_assertions\)\]\s*#\[tauri::command\]/)
  assert.doesNotMatch(host, /#\[tauri::command\]\s*#\[cfg\(debug_assertions\)\]/)
})
