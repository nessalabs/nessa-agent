import assert from "node:assert/strict"
import test from "node:test"
import { hasImmediateCfg, hasNoImmediateCfg } from "./platform-boundaries.mjs"

test("macOS production boundaries require an immediate active cfg attribute", () => {
  assert.equal(
    hasImmediateCfg('#[cfg(target_os = "macos")]\nmod files;', "mod files;"),
    true,
  )
  assert.equal(
    hasImmediateCfg(
      '#[cfg(any(target_os = "macos", test))]\npub(crate) struct Fence;',
      "pub(crate) struct Fence;",
    ),
    true,
  )
  assert.equal(
    hasImmediateCfg('/* #[cfg(target_os = "macos")] */\nmod files;', "mod files;"),
    false,
  )
  assert.equal(hasImmediateCfg("#[cfg(unix)]\nmod files;", "mod files;"), false)
  assert.equal(hasImmediateCfg("mod files;", "mod files;"), false)
})

test("portable runtime identity rejects every immediate target cfg", () => {
  const declaration = "pub(crate) struct RunningRuntime"
  assert.equal(hasNoImmediateCfg("pub(crate) struct RunningRuntime;", declaration), true)
  assert.equal(hasNoImmediateCfg("pub(crate) struct OtherRuntime;", declaration), false)
  assert.equal(
    hasNoImmediateCfg(
      '#[cfg(target_os = "macos")]\npub(crate) struct RunningRuntime;',
      declaration,
    ),
    false,
  )
  assert.equal(
    hasNoImmediateCfg("#[cfg(unix)]\npub(crate) struct RunningRuntime;", declaration),
    false,
  )
})
