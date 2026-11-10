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
  PLATFORM_CODE,
  platformModuleFiles,
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
    text: `"/../../docs/limits.md" "../../../../src/onboarding/adapters/agents.ts" "../../src/keychain/prompt.json"`,
    platformFiles: [
      "crates/nessa-server/src/keychain.rs",
      "crates/nessa-server/src/sandbox.rs",
      "crates/nessa-server/src/sandbox/",
    ],
    platformText: `"tunnel-releases.json" "../../src/keychain/prompt.json"`,
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
    // Cargo unifies features across every listed crate, so one crate's
    // manifest can change how the others build.
    "crates/nessa-timer/Cargo.toml",
    "src-tauri/Cargo.toml",
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
  }
})

test("only an explicit pull request narrows, and nothing else reads the tree", () => {
  for (const event of ["push", "workflow_dispatch", "merge_group"]) {
    const all = planChange([{ filename: "src/styles.css" }], null, { event })
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

test("a Markdown file a Rust test reads reaches that test's crate", () => {
  for (const files of [["docs/limits.md"], ["docs/limits.md", "src/styles.css"]]) {
    const limits = plan(...files)
    assert.equal(limits.code, true)
    assert.deepEqual(limits.rustPackages, ["nessa-server"])
  }
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
  }
  // A platform word in an unchanged context line of a shared file is not a
  // change to it.
  assert.equal(
    plan({
      filename: "crates/nessa-server/src/a.rs",
      patch: "@@ -1 +1 @@\n // unix sockets\n+let a = 2;",
    }).platformTests,
    false,
  )
})

test("any change to a platform file or module runs the platform suites", () => {
  // The edited line names no platform, but the file holds platform code, so
  // the line may sit inside a `#[cfg(windows)]` block.
  for (const filename of [
    "crates/nessa-server/src/keychain.rs",
    "crates/nessa-server/src/sandbox.rs",
    "crates/nessa-server/src/sandbox/rules/apply.rs",
  ]) {
    const changed = plan({ filename, patch: "@@ -1 +1 @@\n-let a = 1;\n+let a = 2;" })
    assert.equal(changed.platformTests, true, filename)
  }
  assert.equal(
    plan({ filename: "crates/nessa-server/src/sandboxes.rs", patch: "+let a = 2;" })
      .platformTests,
    false,
  )
})

test("a data file only platform code reads runs the platform suites", () => {
  assert.equal(plan("crates/nessa-server/data/tunnel-releases.json").platformTests, true)
  assert.equal(plan("crates/nessa-server/data/other.json").platformTests, false)
  assert.equal(plan("src/keychain/prompt.json").platformTests, true)
})

test("removing the last system-crate call is a platform change", () => {
  const removed = plan({
    filename: "crates/nessa-server/src/a.rs",
    patch: "@@ -1 +1 @@\n-    unsafe { windows_sys::Win32::Foundation::CloseHandle(h) };",
  })
  assert.equal(removed.platformTests, true)
})

test("a module declared under a platform cfg marks its file and folder", () => {
  const source = [
    '#[cfg(target_os = "macos")]',
    "mod keychain;",
    "#[cfg(unix)]",
    "#[allow(dead_code)]",
    "pub(crate) mod sockets;",
    "#[cfg(windows)]",
    '#[path = "../tests/windows_acl.rs"]',
    "mod acl;",
    "#[cfg(test)]",
    "mod tests;",
    "mod shared;",
  ].join("\n")
  assert.deepEqual(platformModuleFiles("crates/x/src/lib.rs", source), [
    "crates/x/src/keychain.rs",
    "crates/x/src/keychain/",
    "crates/x/src/sockets.rs",
    "crates/x/src/sockets/",
    "crates/x/tests/windows_acl.rs",
    "crates/x/tests/windows_acl/",
  ])
  // A file that is not `mod.rs` or `lib.rs` usually holds its children in its
  // own folder.
  assert.deepEqual(
    platformModuleFiles("crates/x/src/gateway.rs", "#[cfg(unix)]\nmod peer;"),
    [
      // `gateway.rs` may be a crate root (a `[[bin]]`), so both homes count.
      "crates/x/src/peer.rs",
      "crates/x/src/peer/",
      "crates/x/src/gateway/peer.rs",
      "crates/x/src/gateway/peer/",
    ],
  )
  // rustfmt wraps a long cfg; a comment may sit between it and the module.
  assert.deepEqual(
    platformModuleFiles(
      "crates/x/tests/it.rs",
      '#[cfg(any(\n    target_os = "macos",\n    windows\n))]\n/// Native only.\nmod native;\n#[cfg(test)] mod plain;',
    ),
    [
      "crates/x/tests/native.rs",
      "crates/x/tests/native/",
      "crates/x/tests/it/native.rs",
      "crates/x/tests/it/native/",
    ],
  )
  assert.ok(
    PLATFORM_CODE.test('#[cfg(all(\n    test,\n    target_os = "macos"\n))]\nfn a() {}'),
  )
  assert.ok(!PLATFORM_CODE.test("#[cfg(test)]\nfn a() { let windows = 2; }"))
})

test("a platform-named path or a missing diff runs the platform suites", () => {
  assert.equal(
    plan("crates/nessa-server/src/gateway/macos/staging.rs").platformTests,
    true,
  )
  assert.equal(plan("crates/nessa-server/src/platform_paths.rs").platformTests, true)
  assert.equal(plan({ filename: "crates/nessa-server/src/big.rs" }).platformTests, true)
  // The files API sends `null` for a missing diff or an unrenamed file.
  assert.equal(
    plan({
      filename: "crates/nessa-server/src/big.rs",
      previous_filename: null,
      patch: null,
    }).platformTests,
    true,
  )
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
  const limits = planChange([{ filename: "docs/limits.md", patch: "+a" }], real)
  assert.ok(limits.rustPackages.includes("nessa-server"))
  // `windows-server-tests` is skipped on `platform_tests` alone. That is right
  // only while every tested crate leads to the server.
  for (const name of RUST_TEST_PACKAGES) {
    const reach = new Set([name])
    const queue = [name]
    while (queue.length > 0)
      for (const next of real.dependents.get(queue.pop()) ?? [])
        if (!reach.has(next)) reach.add(next) && queue.push(next)
    assert.ok(reach.has("nessa-server"), name)
  }
  // The platform scan sees the real platform modules.
  const storage = real.members.find((member) => member.name === "nessa-local-storage")
  assert.ok(storage.platformFiles.includes("crates/nessa-local-storage/src/windows/"))
})

test("the workflow builds this script's list and narrows only the Linux run", () => {
  const workflow = readFileSync(
    new URL("../.github/workflows/local-auth.yml", import.meta.url),
    "utf8",
  )
  const full = packageArguments(RUST_TEST_PACKAGES)
  assert.ok(workflow.includes(`cargo clippy ${full} --all-targets`))
  assert.ok(
    workflow.includes(
      `--run-packages "\${{ runner.os == 'Linux' && needs.changes.outputs.rust_packages || '' }}" -- ${full}`,
    ),
  )
  assert.ok(workflow.includes("node scripts/ci-plan.mjs --files-jsonl --github-output"))
  // The macOS and Windows suites, and the steps that only they run, wait on
  // the plan's platform answer.
  assert.match(
    workflow,
    /if: needs\.changes\.outputs\.rust != 'false' && \(runner\.os == 'Linux' \|\| needs\.changes\.outputs\.platform_tests != 'false'\)/,
  )
  assert.match(
    workflow,
    /if: runner\.os == 'Windows' && needs\.changes\.outputs\.platform_tests != 'false'/,
  )
  assert.match(
    workflow,
    /if: runner\.os == 'Windows' && !cancelled\(\) && needs\.changes\.outputs\.platform_tests != 'false'/,
  )
})
