import assert from "node:assert/strict"
import test from "node:test"

import {
  EVERY_PLATFORM,
  MACOS_ONLY,
  declaredModules,
  fileGates,
  modulePaths,
  topLevelItems,
  unreachableOffMacos,
} from "./platform-gates.mjs"

const ROOT = "src/main.rs"

/**
 * The shape of the real crate: a root that uses a module every platform
 * builds, and an adapter inside it only macOS compiles. `rest` adds the lines
 * a case needs to the root, so every fixture keeps `gateway` reached — an
 * unreferenced module is a finding in its own right, and not the one under
 * test.
 */
function crate(extra = {}, rest = "") {
  return new Map(
    Object.entries({
      [ROOT]: `mod gateway;\nuse gateway::Ready;\n${rest}`,
      "src/gateway.rs": `pub struct Ready;\n\n#[cfg(target_os = "macos")]\nmod macos;\n`,
      "src/gateway/macos.rs": `pub fn register() {}\n`,
      ...extra,
    }),
  )
}

test("a module the crate root declares plainly is built everywhere", () => {
  assert.deepEqual(declaredModules("mod gateway;\n"), [
    { name: "gateway", gate: EVERY_PLATFORM },
  ])
})

test("a gate above a module is read through the doc comment between them", () => {
  const source = `#[cfg(target_os = "macos")]\n/// Only macOS registers one.\nmod macos;\n`
  assert.deepEqual(declaredModules(source), [{ name: "macos", gate: MACOS_ONLY }])
})

test("a module's file sits beside its declaration, or in the folder it names", () => {
  assert.deepEqual(modulePaths("src/main.rs", "gateway"), [
    "src/gateway.rs",
    "src/gateway/mod.rs",
  ])
  assert.deepEqual(modulePaths("src/gateway.rs", "macos"), [
    "src/gateway/macos.rs",
    "src/gateway/macos/mod.rs",
  ])
})

test("everything under a macOS-only module is macOS-only, however it is declared", () => {
  const sources = crate({
    "src/gateway/macos.rs": `mod control;\n`,
    "src/gateway/macos/control.rs": ``,
  })
  const gates = fileGates(sources, ROOT)
  assert.equal(gates.get("src/gateway.rs"), EVERY_PLATFORM)
  assert.equal(gates.get("src/gateway/macos.rs"), MACOS_ONLY)
  assert.equal(gates.get("src/gateway/macos/control.rs"), MACOS_ONLY)
})

test("items are the ones at the top level, not the methods inside them", () => {
  const items = topLevelItems(
    `const A: u8 = 1;\nstruct B;\nimpl B {\n    fn inner() {}\n}\n`,
  )
  assert.deepEqual(
    items.map((item) => `${item.kind} ${item.name}`),
    ["const A", "struct B"],
  )
})

/**
 * `stage_port`: a module the root declared plainly, reached only by the launchd
 * registration, which is macOS-only. Windows and Linux compiled every line of
 * it with nothing able to call it.
 */
test("a module only a macOS adapter reaches is reported", () => {
  const sources = crate({
    [ROOT]: `mod gateway;\nuse gateway::Ready;\nmod stage_port;\n`,
    "src/stage_port.rs": `pub fn stage_port(stage: &str) -> Option<u16> { None }\n`,
    "src/gateway/macos.rs": `pub fn register() { crate::stage_port::stage_port("prod"); }\n`,
  })
  assert.deepEqual(
    unreachableOffMacos(sources, ROOT).map((found) => `${found.kind} ${found.name}`),
    ["mod stage_port"],
  )
})

test("the same module gated like its caller is not reported", () => {
  const sources = crate({
    [ROOT]: `mod gateway;\nuse gateway::Ready;\n#[cfg(target_os = "macos")]\nmod stage_port;\n`,
    "src/stage_port.rs": `pub fn stage_port(stage: &str) -> Option<u16> { None }\n`,
    "src/gateway/macos.rs": `pub fn register() { crate::stage_port::stage_port("prod"); }\n`,
  })
  assert.deepEqual(unreachableOffMacos(sources, ROOT), [])
})

/**
 * `KEYCHAIN_SERVICE`: an item beside a macOS-gated function in a file every
 * platform builds. The item needs the gate its only caller carries.
 */
test("an item only a macOS-gated function beside it uses is reported", () => {
  const sources = crate({
    [ROOT]: `mod gateway;\nuse gateway::Ready;\nmod credential;\nuse credential::ready;\n`,
    "src/credential.rs":
      `pub fn ready() {}\n\nconst KEYCHAIN_SERVICE: &str = "nessa";\n\n` +
      `#[cfg(target_os = "macos")]\npub fn store() {\n    let _ = KEYCHAIN_SERVICE;\n}\n`,
  })
  assert.deepEqual(
    unreachableOffMacos(sources, ROOT).map((found) => `${found.kind} ${found.name}`),
    ["const KEYCHAIN_SERVICE"],
  )
})

test("an item every platform's code uses is left alone", () => {
  const sources = crate({
    [ROOT]: `mod gateway;\nuse gateway::Ready;\nmod credential;\nuse credential::ready;\n`,
    "src/credential.rs":
      `pub fn ready() {\n    store();\n}\n\nconst KEYCHAIN_SERVICE: &str = "nessa";\n\n` +
      `pub fn store() {\n    let _ = KEYCHAIN_SERVICE;\n}\n`,
  })
  assert.deepEqual(unreachableOffMacos(sources, ROOT), [])
})

/** The compiler computes dead code for the build that ships, not the test one. */
test("a test is not what keeps an item alive", () => {
  const sources = crate({
    [ROOT]: `mod gateway;\nuse gateway::Ready;\nmod credential;\nuse credential::ready;\n`,
    "src/credential.rs":
      `pub fn ready() {}\n\nconst KEYCHAIN_SERVICE: &str = "nessa";\n\n` +
      `#[cfg(target_os = "macos")]\npub fn store() {\n    let _ = KEYCHAIN_SERVICE;\n}\n\n` +
      `#[cfg(test)]\nmod tests {\n    #[test]\n    fn names_the_service() {\n` +
      `        assert_eq!(super::KEYCHAIN_SERVICE, "nessa");\n    }\n}\n`,
  })
  assert.deepEqual(
    unreachableOffMacos(sources, ROOT).map((found) => found.name),
    ["KEYCHAIN_SERVICE"],
  )
})

test("a name only a comment mentions has not been used", () => {
  const sources = crate({
    [ROOT]: `mod gateway;\nuse gateway::Ready;\nmod credential;\nuse credential::ready;\n`,
    "src/credential.rs":
      `pub fn ready() {}\n\nconst KEYCHAIN_SERVICE: &str = "nessa";\n\n` +
      `// KEYCHAIN_SERVICE is what the macOS keychain is asked for.\n` +
      `#[cfg(target_os = "macos")]\npub fn store() {\n    let _ = KEYCHAIN_SERVICE;\n}\n`,
  })
  assert.deepEqual(
    unreachableOffMacos(sources, ROOT).map((found) => found.name),
    ["KEYCHAIN_SERVICE"],
  )
})

test("the entry point is nobody's caller and is spared", () => {
  const sources = crate({}, `\nfn main() {}\n`)
  assert.deepEqual(unreachableOffMacos(sources, ROOT), [])
})
