import assert from "node:assert/strict"
import {
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
  LINUX_RUNTIME_TARGET,
  prepareLinuxRuntime,
  verifyLinuxRuntimeExecutables,
} from "./prepare-linux.mjs"
import { verifyRuntimeFingerprint } from "./runtime-fingerprint.mjs"
import { runtimeExecutables } from "./runtime-layout.mjs"

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "nessa-prepare-linux-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  for (const [name, contents] of [
    ["target/release/nessa", "gateway"],
    ["target/release/nessa-mcp", "mcp"],
    ["crates/nessa-sdk/data/models.json", '{"models":[]}'],
  ]) {
    const file = join(root, name)
    mkdirSync(dirname(file), { recursive: true })
    writeFileSync(file, contents, { mode: 0o755 })
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
  return root
}

function commands(root, calls) {
  return (command, args, options = {}) => {
    calls.push({ args, command, options })
    if (command === "rustc") return `rustc 1.90.0\nhost: ${LINUX_RUNTIME_TARGET}\n`
    if (command === "cargo" && args[0] === "metadata")
      return JSON.stringify({ target_directory: join(root, "target") })
    if (command === "npm") {
      const bin = join(options.cwd, "node_modules/.bin")
      const packageDirectory = join(options.cwd, "node_modules/package")
      mkdirSync(bin, { recursive: true })
      mkdirSync(packageDirectory, { recursive: true })
      writeFileSync(join(packageDirectory, "index.js"), "entry", { mode: 0o755 })
      symlinkSync("../package/index.js", join(bin, "agent"))
    }
  }
}

function successfulProbe(calls) {
  return (command, args, options) => {
    calls.push({ args, command, options })
    if (command.endsWith("/node")) return { status: 0, stdout: "v26.8.1\n", stderr: "" }
    if (command.endsWith("/nessa")) return { status: 0, stdout: "Nessa\n", stderr: "" }
    return {
      status: 1,
      stdout: "",
      stderr: "expected --workspace PATH --audit-directory PATH",
    }
  }
}

test("Linux assembly publishes and freshly verifies the complete runtime", (t) => {
  const root = fixture(t)
  const commandCalls = []
  const probeCalls = []
  const manifest = prepareLinuxRuntime({
    arch: "x64",
    platform: "linux",
    requestedTarget: LINUX_RUNTIME_TARGET,
    root,
    run: commands(root, commandCalls),
    probe: successfulProbe(probeCalls),
    prepareNode({ executable, out }) {
      writeFileSync(join(out, executable), "node", { mode: 0o755 })
      writeFileSync(join(out, "NODE-LICENSE"), "license")
      return "26.8.1"
    },
  })
  const out = join(root, "src-tauri/runtime")
  assert.equal(manifest.target, LINUX_RUNTIME_TARGET)
  assert.equal(manifest.node, "26.8.1")
  assert.equal(verifyRuntimeFingerprint(out), manifest.fingerprint)
  for (const path of [
    "node",
    "nessa",
    "nessa-mcp",
    "claude-acp/node_modules/.bin/agent",
    "codex-acp/node_modules/.bin/agent",
    "models.json",
    "NODE-LICENSE",
    "manifest.json",
  ])
    assert.equal(typeof readFileSync(join(out, path), "utf8"), "string")
  assert.deepEqual(
    probeCalls.map(({ args, command, options }) => [
      command.split("/").at(-1),
      args,
      options.timeout,
    ]),
    [
      ["node", ["--version"], 5_000],
      ["nessa", ["--help"], 5_000],
      ["nessa-mcp", ["--help"], 5_000],
    ],
  )
})

test("Linux platform, architecture, and requested target refuse before effects", () => {
  for (const options of [
    { platform: "darwin", arch: "x64", requestedTarget: LINUX_RUNTIME_TARGET },
    { platform: "linux", arch: "arm64", requestedTarget: LINUX_RUNTIME_TARGET },
    { platform: "linux", arch: "x64", requestedTarget: "aarch64-unknown-linux-gnu" },
  ]) {
    let effects = 0
    assert.throws(() =>
      prepareLinuxRuntime({
        ...options,
        run() {
          effects += 1
        },
        prepareNode() {
          effects += 1
        },
      }),
    )
    assert.equal(effects, 0)
  }
})

test("Linux requires the native Rust target before build or output mutation", (t) => {
  const root = fixture(t)
  const out = join(root, "src-tauri/runtime")
  mkdirSync(out, { recursive: true })
  writeFileSync(join(out, "manifest.json"), "previous")
  const calls = []
  assert.throws(
    () =>
      prepareLinuxRuntime({
        arch: "x64",
        platform: "linux",
        root,
        run(command) {
          calls.push(command)
          return "rustc 1.90.0\nhost: aarch64-unknown-linux-gnu\n"
        },
      }),
    /native Rust target/,
  )
  assert.deepEqual(calls, ["rustc"])
  assert.equal(readFileSync(join(out, "manifest.json"), "utf8"), "previous")
})

test("Linux executable validation rejects wrong Node and unfinished probes", () => {
  const executables = runtimeExecutables("linux")
  assert.throws(
    () =>
      verifyLinuxRuntimeExecutables({
        executables,
        out: "/runtime",
        probe: () => ({ status: 0, stdout: "v26.8.0\n", stderr: "" }),
      }),
    /did not report v26.8.1/,
  )
  assert.throws(
    () =>
      verifyLinuxRuntimeExecutables({
        executables,
        out: "/runtime",
        probe: () => ({ status: null, signal: "SIGTERM", stdout: "", stderr: "" }),
      }),
    /did not finish its bounded probe/,
  )
})
