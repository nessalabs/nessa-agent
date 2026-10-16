/** Identity publication and omission detection exercise generated Rust directly. */
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"
import { methodIdentities, methodIdentityRust } from "./method-identity.mjs"

const root = fileURLToPath(new URL("../../", import.meta.url))
const manifest = JSON.parse(
  readFileSync(join(root, "protocol/product/manifest.json"), "utf8"),
)
const names = Object.keys(manifest.methods)

function fixture(run) {
  const path = mkdtempSync(join(tmpdir(), "nessa-method-identity-"))
  try {
    run(path)
  } finally {
    rmSync(path, { recursive: true, force: true })
  }
}
function compile(path, source) {
  writeFileSync(join(path, "main.rs"), source)
  const result = spawnSync(
    process.env.RUSTC ?? "rustc",
    [
      "--edition=2021",
      "--error-format=json",
      join(path, "main.rs"),
      "-o",
      join(path, "probe"),
    ],
    { encoding: "utf8", timeout: 30_000 },
  )
  assert.equal(result.error, undefined)
  return result
}
function classifierSource(path, methods) {
  writeFileSync(join(path, "identity.rs"), methodIdentityRust(Object.keys(methods)))
  return `#![allow(dead_code)]
extern crate self as nessa_protocol;
pub mod product { pub mod generated { include!(${JSON.stringify(join(path, "identity.rs"))}); } }
#[path = ${JSON.stringify(join(root, "crates/nessa-server/src/product/method_plan.rs"))}]
mod method_plan;
fn main() {}
`
}

test("manifest names have unique PascalCase Rust identities", () => {
  const identities = methodIdentities(names)
  assert.equal(identities.length, names.length)
  assert.equal(new Set(identities.map(({ variant }) => variant)).size, names.length)
  for (const [name, variant] of [
    ["session.authenticate", "SessionAuthenticate"],
    ["conversation.answerQuestion", "ConversationAnswerQuestion"],
    ["mcp.callTool", "McpCallTool"],
    ["mcpServers.list", "McpServersList"],
  ])
    assert.equal(identities.find((identity) => identity.name === name)?.variant, variant)
})

test("ambiguous or invalid Rust identities refuse publication", () => {
  for (const names of [
    ["a.b", "aB"],
    ["a.b", "A.b"],
    ["a.b", "a.b"],
  ])
    assert.throws(() => methodIdentityRust(names), /collides/)
  for (const name of [
    "",
    "a..b",
    ".a",
    "a.",
    "a-b",
    "3a.b",
    "a.3b",
    "self",
    'a."b',
    null,
  ])
    assert.throws(
      () => methodIdentityRust([name]),
      /Invalid product method Rust identity/,
    )
})

test("compiled identity round trips every manifest name and refuses unknown or changed case", () =>
  fixture((path) => {
    writeFileSync(join(path, "identity.rs"), methodIdentityRust(names))
    const result = compile(
      path,
      `#![deny(missing_docs)]
//! Generated identity behavior fixture.
include!("identity.rs");
fn require_traits<T: Copy + Clone + std::fmt::Debug + Eq + PartialEq + std::hash::Hash>() {}
fn main() {
    require_traits::<ProductMethod>();
    let expected = [${names.map(JSON.stringify).join(",")}];
    assert_eq!(ProductMethod::ALL.len(), expected.len());
    for (method, name) in ProductMethod::ALL.iter().zip(expected) {
        assert_eq!(method.as_str(), name);
        assert_eq!(ProductMethod::from_name(name), Some(*method));
        assert_eq!(ProductMethod::from_name(&name.to_uppercase()), None);
        assert_eq!(ProductMethod::from_name(&format!("{} ", name)), None);
    }
    for unknown in ["", "unknown.method", "conversation", "Conversation.send", "conversation.Send"] {
        assert_eq!(ProductMethod::from_name(unknown), None);
    }
}
`,
    )
    assert.equal(result.status, 0, result.stderr)
    const run = spawnSync(join(path, "probe"), [], { encoding: "utf8", timeout: 30_000 })
    assert.equal(run.error, undefined)
    assert.equal(run.status, 0, run.stderr)
  }))

test("a synthetic granted method cannot inherit the real gateway classifier", () =>
  fixture((path) => {
    // First prove the unchanged real classifier compiles with current identities;
    // unrelated fixture errors cannot satisfy the expected extension rejection.
    const baseline = compile(path, classifierSource(path, manifest.methods))
    assert.equal(baseline.status, 0, baseline.stderr)
    const extended = {
      ...manifest.methods,
      "extensionProbe.execute": { params: null, grant: "conversation.write" },
    }
    const result = compile(path, classifierSource(path, extended))
    assert.notEqual(result.status, 0)
    const diagnostics = result.stderr
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line))
    assert.ok(
      diagnostics.some(
        (diagnostic) =>
          diagnostic.code?.code === "E0004" &&
          diagnostic.message.includes("ExtensionProbeExecute"),
      ),
      result.stderr,
    )
  }))
