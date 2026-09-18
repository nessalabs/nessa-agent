import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"

import { agentBlock, namespaceRoot } from "./dev-agent-config.mjs"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const script = join(root, "scripts/dev-agent-config.mjs")

/** Run the script against an isolated data root; never the developer's own. */
function run(dataDir, extra = {}) {
  return execFileSync("node", [script], {
    cwd: root,
    encoding: "utf8",
    env: { ...process.env, NESSA_DATA_DIR: dataDir, NESSA_STAGE: "dev", ...extra },
  })
}

function temporaryRoot() {
  return mkdtempSync(join(tmpdir(), "nessa-dev-agent-"))
}

test("the namespace matches the server's stage and instance layout", () => {
  assert.equal(
    namespaceRoot({ NESSA_DATA_DIR: "/data", NESSA_STAGE: "dev" }),
    "/data/dev",
  )
  assert.equal(namespaceRoot({ NESSA_DATA_DIR: "/data", NESSA_STAGE: "prod" }), "/data")
  assert.equal(
    namespaceRoot({ NESSA_DATA_DIR: "/data", NESSA_STAGE: "dev", NESSA_INSTANCE: "wt" }),
    "/data/dev/instances/wt",
  )
  assert.equal(namespaceRoot({}, "/home/dev"), "/home/dev/.nessa/dev")
  for (const instance of ["..", "", "one/two", "one two"])
    assert.throws(() =>
      namespaceRoot({ NESSA_DATA_DIR: "/data", NESSA_INSTANCE: instance }),
    )
  assert.throws(() => namespaceRoot({ NESSA_DATA_DIR: "relative" }))
})

test("every configured path is absolute and the MCP server is opt-in", () => {
  const without = agentBlock({
    checkout: "/checkout",
    namespace: "/data/dev",
    node: "/usr/bin/node",
    mcpBinary: undefined,
  })
  for (const key of ["catalog", "node", "acpEntry", "workspace"])
    assert.ok(without[key].startsWith("/"), `${key} is absolute`)
  assert.equal(without.mcpServers, undefined)
  assert.equal(without.workspace, "/data/dev/workspaces/default")
  const with_ = agentBlock({
    checkout: "/checkout",
    namespace: "/data/dev",
    node: "/usr/bin/node",
    mcpBinary: "/checkout/target/debug/nessa-mcp",
  })
  assert.equal(with_.mcpServers[0].command, "/checkout/target/debug/nessa-mcp")
  assert.deepEqual(with_.mcpServers[0].args, [
    "--workspace",
    "/data/dev/workspaces/default",
    "--audit-directory",
    "/data/dev/process-audit",
  ])
})

test("a fresh namespace gets a private config the workspace of which exists", (t) => {
  if (process.platform === "win32")
    return t.skip("Claude ACP needs Unix process supervision")
  if (!existsSync(join(root, "crates/nessa-sdk/harnesses/claude-acp/node_modules")))
    return t.skip("the Claude ACP harness is not installed in this checkout")
  const data = temporaryRoot()
  const output = run(data)
  assert.match(output, /dev agent configured/)
  const path = join(data, "dev/config.json")
  const config = JSON.parse(readFileSync(path, "utf8"))
  assert.equal(config.agent.model, "claude-sonnet-5")
  assert.ok(existsSync(config.agent.node))
  assert.ok(existsSync(config.agent.acpEntry))
  assert.ok(existsSync(config.agent.catalog))
  assert.ok(statSync(config.agent.workspace).isDirectory())
  // The gateway refuses to read anything under this root that others can see.
  assert.equal(statSync(path).mode & 0o077, 0)
  assert.equal(statSync(config.agent.workspace).mode & 0o077, 0)

  // Running the dev loop again must not churn the file.
  const before = readFileSync(path, "utf8")
  assert.match(run(data), /already configured/)
  assert.equal(readFileSync(path, "utf8"), before)
})

test("settings a developer wrote are preserved, and their own agent is never replaced", (t) => {
  if (process.platform === "win32")
    return t.skip("Claude ACP needs Unix process supervision")
  if (!existsSync(join(root, "crates/nessa-sdk/harnesses/claude-acp/node_modules")))
    return t.skip("the Claude ACP harness is not installed in this checkout")
  const data = temporaryRoot()
  mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
  const path = join(data, "dev/config.json")
  writeFileSync(path, JSON.stringify({ session: { writeTimeoutMs: 75 } }), {
    mode: 0o600,
  })
  run(data)
  const merged = JSON.parse(readFileSync(path, "utf8"))
  assert.equal(merged.session.writeTimeoutMs, 75)
  assert.ok(merged.agent)

  const mine = { agent: { node: "/nowhere/node", acpEntry: "/nowhere/index.js" } }
  writeFileSync(path, JSON.stringify(mine), { mode: 0o600 })
  const output = run(data)
  assert.deepEqual(JSON.parse(readFileSync(path, "utf8")), mine)
  // A hand-written block that cannot launch is reported rather than repaired.
  assert.match(output, /points at files that are not there/)
  assert.match(output, /\/nowhere\/node/)
})

test("a config that does not parse is reported, not overwritten", () => {
  const data = temporaryRoot()
  mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
  const path = join(data, "dev/config.json")
  writeFileSync(path, "{ not json", { mode: 0o600 })
  const output = run(data)
  assert.match(output, /not valid JSON/)
  assert.equal(readFileSync(path, "utf8"), "{ not json")
})

test("a clone with no harness installed is told what to run, and writes nothing", (t) => {
  if (process.platform === "win32")
    return t.skip("Claude ACP needs Unix process supervision")
  // A copy of this script with the checked-in catalog beside it and no
  // node_modules: exactly what someone has a minute after `git clone`.
  const checkout = mkdtempSync(join(tmpdir(), "nessa-fresh-clone-"))
  mkdirSync(join(checkout, "scripts"), { recursive: true })
  mkdirSync(join(checkout, "crates/nessa-sdk/data"), { recursive: true })
  copyFileSync(script, join(checkout, "scripts/dev-agent-config.mjs"))
  copyFileSync(
    join(root, "crates/nessa-sdk/data/models.json"),
    join(checkout, "crates/nessa-sdk/data/models.json"),
  )
  const data = temporaryRoot()
  const output = execFileSync("node", [join(checkout, "scripts/dev-agent-config.mjs")], {
    encoding: "utf8",
    env: { ...process.env, NESSA_DATA_DIR: data, NESSA_STAGE: "dev" },
  })
  assert.match(output, /harness is not installed/)
  assert.match(output, /npm ci --omit=dev/)
  assert.equal(existsSync(join(data, "dev/config.json")), false)
})
