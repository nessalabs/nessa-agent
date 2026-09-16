import assert from "node:assert/strict"
import {
  chmodSync,
  cpSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  renameSync,
  rmSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, relative } from "node:path"
import test from "node:test"
import { runtimeFingerprint, verifyRuntimeFingerprint } from "./runtime-fingerprint.mjs"
import { materializeBinLinks } from "./materialize-bin-links.mjs"

function fixture(t, reverse = false) {
  const root = mkdtempSync(join(tmpdir(), "nessa-fingerprint-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const files = [
    ["nessa", "gateway"],
    ["nessa-mcp", "tools"],
    ["node", "runtime"],
    ["models.json", '{"models":[]}'],
    ["claude-acp/package-lock.json", "lock"],
    [
      "claude-acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js",
      "adapter",
    ],
    ["claude-acp/node_modules/dependency/index.js", "dependency"],
    [
      "claude-acp/node_modules/@anthropic-ai/claude-agent-sdk-darwin-arm64/claude",
      "provider",
    ],
  ]
  for (const [name, content] of reverse ? files.toReversed() : files) {
    const path = join(root, name)
    mkdirSync(dirname(path), { recursive: true })
    writeFileSync(path, content, { mode: 0o644 })
  }
  return { root, files }
}

test("fingerprints include every shipped model, executable, adapter and dependency", (t) => {
  const { root, files } = fixture(t)
  const original = runtimeFingerprint(root)
  for (const [name, content] of files) {
    writeFileSync(join(root, name), content + "changed")
    assert.notEqual(runtimeFingerprint(root), original, name)
    writeFileSync(join(root, name), content)
    assert.equal(runtimeFingerprint(root), original)
  }
  writeFileSync(join(root, "new-resource"), "new")
  assert.notEqual(runtimeFingerprint(root), original)
  rmSync(join(root, "new-resource"))
  rmSync(join(root, "models.json"))
  assert.notEqual(runtimeFingerprint(root), original)
})

test("locations, creation order, timestamps and the generated manifest are irrelevant", (t) => {
  const first = fixture(t).root
  const second = fixture(t, true).root
  const original = runtimeFingerprint(first)
  assert.equal(runtimeFingerprint(second), original)
  writeFileSync(join(first, "manifest.json"), '{"fingerprint":"previous"}')
  utimesSync(join(first, "node"), 100, 100)
  assert.equal(runtimeFingerprint(first), original)
  writeFileSync(join(first, "claude-acp/manifest.json"), "dependency manifest")
  assert.notEqual(runtimeFingerprint(first), original)
})

test("paths, executable permissions and byte boundaries are part of identity", (t) => {
  const { root } = fixture(t)
  const original = runtimeFingerprint(root)
  renameSync(join(root, "models.json"), join(root, "renamed.json"))
  assert.notEqual(runtimeFingerprint(root), original)
  renameSync(join(root, "renamed.json"), join(root, "models.json"))
  chmodSync(join(root, "node"), 0o755)
  assert.notEqual(runtimeFingerprint(root), original)
  chmodSync(join(root, "node"), 0o644)
  writeFileSync(join(root, "nessa"), "a")
  writeFileSync(join(root, "nessa-mcp"), "bc")
  const framed = runtimeFingerprint(root)
  writeFileSync(join(root, "nessa"), "ab")
  writeFileSync(join(root, "nessa-mcp"), "c")
  assert.notEqual(runtimeFingerprint(root), framed)
})

test("internal symlinks identify their target and cannot depend on external files", (t) => {
  const { root } = fixture(t)
  const link = join(root, "node-link")
  symlinkSync("node", link)
  const original = runtimeFingerprint(root)
  rmSync(link)
  symlinkSync("nessa", link)
  assert.notEqual(runtimeFingerprint(root), original)
  rmSync(link)
  symlinkSync(join(root, "node"), link)
  assert.throws(() => runtimeFingerprint(root), /inside the bundle/)
  rmSync(link)
  const external = fixture(t).root
  symlinkSync(relative(root, join(external, "node")), link)
  assert.throws(() => runtimeFingerprint(root), /inside the bundle/)
  rmSync(link)
  symlinkSync("missing", link)
  assert.throws(() => runtimeFingerprint(root), /ENOENT/)
})

test("prepared npm launchers keep their identity when the app bundle copies them", (t) => {
  const root = mkdtempSync(join(tmpdir(), "nessa-runtime-links-"))
  const bundled = mkdtempSync(join(tmpdir(), "nessa-runtime-bundle-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  t.after(() => rmSync(bundled, { recursive: true, force: true }))
  const modules = join(root, "node_modules")
  mkdirSync(join(modules, ".bin"), { recursive: true })
  mkdirSync(join(modules, "package"), { recursive: true })
  writeFileSync(join(modules, "package/cli.js"), "#!/usr/bin/env node\n", {
    mode: 0o755,
  })
  symlinkSync("../package/cli.js", join(modules, ".bin/package"))

  materializeBinLinks(modules)
  assert.equal(lstatSync(join(modules, ".bin/package")).isFile(), true)
  cpSync(root, bundled, { recursive: true, dereference: true })
  assert.equal(runtimeFingerprint(root), runtimeFingerprint(bundled))
})

test("final runtime verification rejects content changed after manifest generation", (t) => {
  const { root } = fixture(t)
  const fingerprint = runtimeFingerprint(root)
  writeFileSync(join(root, "manifest.json"), JSON.stringify({ fingerprint }))
  assert.equal(verifyRuntimeFingerprint(root), fingerprint)
  writeFileSync(join(root, "models.json"), "changed after packaging")
  assert.throws(() => verifyRuntimeFingerprint(root), /does not match its manifest/)
})
