import assert from "node:assert/strict"
import {
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import test from "node:test"

import {
  assembleDesktopRuntime,
  runtimeExecutableNames,
  rustHostTarget,
} from "./prepare-runtime.mjs"
import { verifyRuntimeFingerprint } from "./runtime-fingerprint.mjs"
import { RUNTIME_EXECUTABLES } from "./runtime-signing.mjs"

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "nessa-prepare-runtime-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const target = join(root, "target")
  const out = join(root, "src-tauri/runtime")
  for (const [name, contents] of [
    ["target/release/nessa", "gateway"],
    ["target/release/nessa-mcp", "mcp"],
    ["crates/nessa-sdk/data/models.json", '{"models":[]}'],
  ]) {
    const file = join(root, name)
    mkdirSync(dirname(file), { recursive: true })
    writeFileSync(file, contents)
  }
  for (const [name, dependency, version] of [
    ["claude-acp", "@agentclientprotocol/claude-agent-acp", "0.76.0"],
    ["codex-acp", "@agentclientprotocol/codex-acp", "1.12.0"],
  ]) {
    const harness = join(root, "crates/nessa-sdk/harnesses", name)
    mkdirSync(harness, { recursive: true })
    writeFileSync(
      join(harness, "package.json"),
      JSON.stringify({ private: true, dependencies: { [dependency]: version } }),
    )
    writeFileSync(join(harness, "package-lock.json"), "{}")
  }
  return { out, root, target }
}

function fakeCommands(target, calls) {
  return (command, args, options = {}) => {
    calls.push({ args, command, options })
    if (command === "rustc") return "rustc 1.90.0\nhost: aarch64-apple-darwin\n"
    if (command === "cargo" && args[0] === "metadata")
      return JSON.stringify({ target_directory: target })
    if (command === "npm") {
      const bin = join(options.cwd, "node_modules/.bin")
      const packageDirectory = join(options.cwd, "node_modules/package")
      mkdirSync(bin, { recursive: true })
      mkdirSync(packageDirectory, { recursive: true })
      writeFileSync(join(packageDirectory, "index.js"), "entry")
      symlinkSync("../package/index.js", join(bin, "agent"))
    }
  }
}

test("desktop platform layouts name their bundled executables", () => {
  assert.deepEqual(runtimeExecutableNames("darwin"), {
    node: "node",
    gateway: "nessa",
    mcp: "nessa-mcp",
  })
  assert.deepEqual(runtimeExecutableNames("linux"), runtimeExecutableNames("darwin"))
  assert.deepEqual(runtimeExecutableNames("win32"), {
    node: "node.exe",
    gateway: "nessa.exe",
    mcp: "nessa-mcp.exe",
  })
  assert.deepEqual(Object.values(runtimeExecutableNames("darwin")), RUNTIME_EXECUTABLES)
  assert.throws(() => runtimeExecutableNames("freebsd"), /does not support freebsd/)
})

test("the Rust host target must be present and exact", () => {
  assert.equal(
    rustHostTarget("rustc 1.90.0\nbinary: rustc\nhost: x86_64-unknown-linux-gnu\n"),
    "x86_64-unknown-linux-gnu",
  )
  assert.throws(() => rustHostTarget("rustc 1.90.0\n"), /did not report its host target/)
})

test("assembly publishes one complete relocatable runtime manifest", (t) => {
  const { out, root, target } = fixture(t)
  const calls = []
  mkdirSync(out, { recursive: true })
  writeFileSync(join(out, "removed-resource"), "stale")
  const node = join(root, "downloaded-node")
  const license = join(root, "downloaded-node-license")
  writeFileSync(node, "node")
  writeFileSync(license, "license")

  const manifest = assembleDesktopRuntime({
    root,
    out,
    platform: "darwin",
    requestedTarget: "aarch64-apple-darwin",
    run: fakeCommands(target, calls),
    prepareNode({ executable, out: runtime }) {
      cpSync(node, join(runtime, executable))
      cpSync(license, join(runtime, "NODE-LICENSE"))
      return "26.8.1"
    },
    finalizeExecutables({ executables, out: runtime }) {
      for (const name of Object.values(executables))
        assert.equal(typeof readFileSync(join(runtime, name), "utf8"), "string")
      assert.equal(readFileSync(join(runtime, "models.json"), "utf8"), '{"models":[]}')
    },
  })

  assert.deepEqual(manifest, {
    node: "26.8.1",
    claudeAcp: "0.76.0",
    codexAcp: "1.12.0",
    target: "aarch64-apple-darwin",
    fingerprint: verifyRuntimeFingerprint(out),
  })
  assert.deepEqual(JSON.parse(readFileSync(join(out, "manifest.json"), "utf8")), manifest)
  assert.throws(() => readFileSync(join(out, "removed-resource")), /ENOENT/)
  assert.equal(
    readFileSync(join(out, "claude-acp/node_modules/.bin/agent"), "utf8"),
    "entry",
  )
  assert.equal(
    readFileSync(join(out, "codex-acp/node_modules/.bin/agent"), "utf8"),
    "entry",
  )
  assert.deepEqual(
    calls.map(({ args, command }) => [command, args[0]]),
    [
      ["rustc", "-vV"],
      ["cargo", "build"],
      ["cargo", "metadata"],
      ["npm", "ci"],
      ["npm", "ci"],
    ],
  )
})

test("a mismatched target leaves the last complete runtime untouched", (t) => {
  const { out, root, target } = fixture(t)
  mkdirSync(out, { recursive: true })
  writeFileSync(join(out, "manifest.json"), '{"fingerprint":"previous"}')
  const calls = []

  assert.throws(
    () =>
      assembleDesktopRuntime({
        root,
        out,
        platform: "darwin",
        requestedTarget: "x86_64-apple-darwin",
        run: fakeCommands(target, calls),
        prepareNode() {
          assert.fail("Node preparation ran for a mismatched target")
        },
        finalizeExecutables() {
          assert.fail("finalization ran for a mismatched target")
        },
      }),
    /target architecture/,
  )
  assert.equal(
    readFileSync(join(out, "manifest.json"), "utf8"),
    '{"fingerprint":"previous"}',
  )
  assert.deepEqual(
    calls.map(({ command }) => command),
    ["rustc"],
  )
})

test("invalid cargo metadata leaves the last complete runtime untouched", (t) => {
  const { out, root, target } = fixture(t)
  mkdirSync(out, { recursive: true })
  writeFileSync(join(out, "manifest.json"), '{"fingerprint":"previous"}')
  const run = fakeCommands(target, [])

  assert.throws(
    () =>
      assembleDesktopRuntime({
        root,
        out,
        platform: "darwin",
        run(command, args, options) {
          if (command === "cargo" && args[0] === "metadata") return "{}"
          return run(command, args, options)
        },
        prepareNode() {
          assert.fail("Node preparation ran with invalid cargo metadata")
        },
        finalizeExecutables() {
          assert.fail("finalization ran with invalid cargo metadata")
        },
      }),
    /did not report its target directory/,
  )
  assert.equal(
    readFileSync(join(out, "manifest.json"), "utf8"),
    '{"fingerprint":"previous"}',
  )
})

test("failed platform finalization cannot publish a runtime manifest", (t) => {
  const { out, root, target } = fixture(t)

  assert.throws(
    () =>
      assembleDesktopRuntime({
        root,
        out,
        platform: "darwin",
        run: fakeCommands(target, []),
        prepareNode({ executable, out: runtime }) {
          writeFileSync(join(runtime, executable), "node")
          writeFileSync(join(runtime, "NODE-LICENSE"), "license")
          return "26.8.1"
        },
        finalizeExecutables() {
          throw new Error("signing refused")
        },
      }),
    /signing refused/,
  )
  assert.throws(() => readFileSync(join(out, "manifest.json")), /ENOENT/)
})
