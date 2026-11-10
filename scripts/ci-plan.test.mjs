import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"

import {
  FILES_API_LIMIT,
  RUST_TEST_PACKAGES,
  githubOutput,
  memberManifest,
  packageArguments,
  planChange,
  readWorkspace,
  stringLiterals,
  workspaceGraph,
  workspaceMembers,
} from "./ci-plan.mjs"

/**
 * This plan decides which Rust suites a pull request runs. Every case here is
 * about the direction it may be wrong in: a suite that runs and could not have
 * failed costs minutes, and a suite skipped that would have failed is silent.
 * So most cases are about what still reaches everything.
 */

const manifest = (name, deps = []) =>
  `[package]\nname = "${name}"\n\n[dependencies]\n${deps
    .map((dep) => `${dep} = { path = "../${dep}" }`)
    .join("\n")}\n`

// A small workspace with the real crates' shape: storage at the bottom, the
// SDK over it, the protocol over the SDK, the server over everything, and a
// desktop crate that reads a frontend file.
const graph = workspaceGraph([
  {
    dir: "crates/nessa-local-storage",
    manifest: manifest("nessa-local-storage"),
    text: "",
  },
  { dir: "crates/nessa-timer", manifest: manifest("nessa-timer"), text: "" },
  {
    dir: "crates/nessa-sdk",
    manifest: manifest("nessa-sdk", ["nessa-local-storage", "nessa-timer"]),
    text: "",
  },
  {
    dir: "crates/nessa-protocol",
    manifest: manifest("nessa-protocol", ["nessa-sdk"]),
    text: "",
  },
  {
    dir: "crates/nessa-server",
    manifest: manifest("nessa-server", ["nessa-sdk", "nessa-protocol", "nessa-timer"]),
    text: `"../../../../src/onboarding/adapters/agents.ts"`,
  },
  {
    dir: "src-tauri",
    manifest: `[package]\nname = "nessa-app"\n\n[dependencies]\nnessa-local-storage = { path = "../crates/nessa-local-storage" }\n`,
    text: `"../../src/host/window.ts" "../src/desktop/styles.css"`,
  },
])

const plan = (...files) =>
  planChange(
    files.map((file) =>
      typeof file === "string" ? { filename: file, patch: "+ok" } : file,
    ),
    graph,
  )

test("a change with no Rust consequence runs no Rust suite", () => {
  const ui = plan("src/desktop/workspace/ui/transcript/transcript.tsx", "src/styles.css")
  assert.equal(ui.code, true)
  assert.equal(ui.rust, false)
  assert.deepEqual(ui.rustPackages, [])
  assert.equal(ui.sdk, false)
  assert.equal(ui.platformTests, false)
  assert.equal(ui.windowsServerTests, false)
})

test("a frontend file the desktop crate embeds reaches only the desktop crate", () => {
  // nessa-app is outside the nine-crate test list; its own checks are the
  // desktop jobs, which this plan never skips.
  const embedded = plan("src/host/window.ts")
  assert.equal(embedded.rust, false)
  assert.ok(embedded.reasons.some((reason) => reason.includes("named by nessa-app")))
})

test("a frontend file a server test reads reaches the server", () => {
  const read = plan("src/onboarding/adapters/agents.ts")
  assert.deepEqual(read.rustPackages, ["nessa-server"])
  assert.equal(read.sdk, false)
})

test("a crate change reaches that crate and everything that depends on it", () => {
  assert.deepEqual(plan("crates/nessa-server/src/webhooks/mod.rs").rustPackages, [
    "nessa-server",
  ])
  assert.deepEqual(plan("crates/nessa-sdk/src/lib.rs").rustPackages, [
    "nessa-server",
    "nessa-protocol",
    "nessa-sdk",
  ])
  const storage = plan("crates/nessa-local-storage/src/lib.rs")
  assert.deepEqual(storage.rustPackages, [
    "nessa-local-storage",
    "nessa-server",
    "nessa-protocol",
    "nessa-sdk",
  ])
  assert.equal(storage.sdk, true)
})

test("a renamed file counts at both its old and new path", () => {
  const moved = plan({
    filename: "crates/nessa-server/src/a.rs",
    previous_filename: "crates/nessa-sdk/src/a.rs",
    patch: "",
  })
  assert.ok(moved.rustPackages.includes("nessa-sdk"))
})

test("anything outside the members and the frontend roots reaches every crate", () => {
  for (const path of [
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain.toml",
    "protocol/product/manifest.json",
    "scripts/cargo-test-parallel.mjs",
    ".github/workflows/local-auth.yml",
    "justfile",
    "some/new/folder/file.txt",
  ]) {
    const all = plan(path)
    assert.deepEqual(all.rustPackages, RUST_TEST_PACKAGES, path)
    assert.equal(all.sdk, true, path)
    assert.equal(all.platformTests, true, path)
    assert.equal(all.windowsServerTests, true, path)
  }
})

test("only an explicit pull request narrows", () => {
  for (const event of ["push", "workflow_dispatch", "merge_group"]) {
    const all = planChange([{ filename: "src/styles.css" }], graph, { event })
    assert.deepEqual(all.rustPackages, RUST_TEST_PACKAGES)
    assert.equal(all.platformTests, true)
  }
})

test("an empty or possibly cut file list runs everything", () => {
  assert.equal(planChange([], graph).platformTests, true)
  const many = Array.from({ length: FILES_API_LIMIT }, (_, index) => ({
    filename: `src/file-${index}.ts`,
  }))
  assert.deepEqual(planChange(many, graph).rustPackages, RUST_TEST_PACKAGES)
})

test("Markdown alone still skips everything, as documentation-only says", () => {
  const docs = plan("README.md", "crates/nessa-sdk/docs/claude-acp.md")
  assert.equal(docs.code, false)
  assert.equal(docs.rust, false)
})

test("macOS and Windows test suites run only for platform code", () => {
  assert.equal(
    plan({
      filename: "crates/nessa-server/src/a.rs",
      patch: "@@ -1 +1 @@\n-let a = 1;\n+let a = 2;",
    }).platformTests,
    false,
  )
  for (const line of [
    "+#[cfg(unix)]",
    "-#[cfg(windows)]",
    '+    if cfg!(target_os = "macos") {',
    "+use std::os::unix::fs::PermissionsExt;",
    "+#[cfg(not(test))]",
  ]) {
    const changed = plan({
      filename: "crates/nessa-server/src/a.rs",
      patch: `@@ -1 +1 @@\n${line}`,
    })
    assert.equal(changed.platformTests, true, line)
    assert.equal(changed.windowsServerTests, true, line)
  }
  // A platform word in an unchanged context line is not a change to it.
  assert.equal(
    plan({
      filename: "crates/nessa-server/src/a.rs",
      patch: "@@ -1 +1 @@\n #[cfg(unix)]\n+let a = 2;",
    }).platformTests,
    false,
  )
})

test("a platform-named path or a missing diff runs the platform suites", () => {
  assert.equal(
    plan("crates/nessa-server/src/gateway/macos/staging.rs").platformTests,
    true,
  )
  assert.equal(plan("crates/nessa-server/src/platform_paths.rs").platformTests, true)
  assert.equal(plan({ filename: "crates/nessa-server/src/big.rs" }).platformTests, true)
  assert.equal(plan({ filename: "crates/nessa-server/Cargo.toml" }).platformTests, true)
})

test("platform code outside the server runs local-auth's suites, not the server partitions", () => {
  const sdkOnly = planChange(
    [{ filename: "crates/nessa-timer/src/lib.rs", patch: "+#[cfg(unix)]" }],
    workspaceGraph([
      { dir: "crates/nessa-timer", manifest: manifest("nessa-timer"), text: "" },
      { dir: "crates/nessa-server", manifest: manifest("nessa-server"), text: "" },
    ]),
  )
  assert.equal(sdkOnly.platformTests, true)
  assert.equal(sdkOnly.windowsServerTests, false)
})

test("a comment that cites a path does not count as reading it", () => {
  assert.deepEqual(
    stringLiterals(
      '/// See `verification/desktop/scripts/stress.mjs`.\n// "src/x.ts"\nlet a = include_str!("../src/host/window.ts"); let b = "a\\"b";',
    ),
    ['"../src/host/window.ts"', '"a\\"b"'],
  )
})

test("manifests are read for members and every kind of path dependency", () => {
  assert.deepEqual(
    workspaceMembers('[workspace]\nmembers = [\n  "crates/a", "src-tauri/"]\n'),
    ["crates/a", "src-tauri"],
  )
  assert.deepEqual(
    memberManifest(
      '[package]\nname = "x"\n[dependencies]\na = { path = "../a" }\n[dev-dependencies]\nb = { path = "../b", features = ["t"] }\n[build-dependencies]\nc = { path = "../c" }\n',
    ),
    { name: "x", paths: ["../a", "../b", "../c"] },
  )
})

test("the GitHub output spells every answer, the package list as cargo takes it", () => {
  const output = githubOutput(plan("crates/nessa-server/src/a.rs"))
  assert.equal(
    output,
    [
      "code=true",
      "rust=true",
      "rust_packages=-p nessa-server",
      "sdk=false",
      "platform_tests=false",
      "windows_server_tests=false",
      "",
    ].join("\n"),
  )
  assert.equal(packageArguments([]), "")
})

test("the real workspace reads, and every tested crate is a member", () => {
  const real = readWorkspace(new URL("..", import.meta.url).pathname)
  const names = real.members.map((member) => member.name)
  for (const name of RUST_TEST_PACKAGES) assert.ok(names.includes(name), name)
  // Facts the narrowing relies on, checked against the tree rather than assumed.
  assert.ok(real.dependents.get("nessa-sdk").has("nessa-server"))
  assert.ok(real.dependents.get("nessa-sdk").has("nessa-protocol"))
  const ui = planChange(
    [{ filename: "src/desktop/workspace/ui/transcript/transcript.tsx" }],
    real,
  )
  assert.equal(ui.rust, false)
  const host = planChange([{ filename: "src/host/window.ts" }], real)
  assert.equal(host.rust, false)
  assert.ok(host.reasons.some((reason) => reason.includes("nessa-app")))
})

test("the workflow's fallback list is this script's list", () => {
  const workflow = readFileSync(
    new URL("../.github/workflows/local-auth.yml", import.meta.url),
    "utf8",
  )
  assert.ok(workflow.includes(`'${packageArguments(RUST_TEST_PACKAGES)}'`))
  assert.ok(workflow.includes("node scripts/ci-plan.mjs --files-jsonl --github-output"))
})
