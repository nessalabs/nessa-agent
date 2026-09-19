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

/**
 * Configuring a dev agent is a Unix capability — `dev-agent-config.mjs` says so
 * itself and writes nothing on Windows — so there is nothing here for a Windows
 * checkout to be right or wrong about. That covers the paths as much as the
 * writing: they are built with POSIX separators for the machine that would run
 * the agent, and asserting them against `win32` semantics would be testing a
 * shape this script never produces there.
 */
const unixOnly =
  process.platform === "win32"
    ? { skip: "Claude ACP needs Unix process supervision" }
    : {}

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

test("the namespace matches the server's stage and instance layout", unixOnly, () => {
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

test("every configured path is absolute and the MCP server is opt-in", unixOnly, () => {
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

test(
  "a fresh namespace gets a private config the workspace of which exists",
  unixOnly,
  (t) => {
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
  },
)

test(
  "settings a developer wrote are preserved, and their own agent is never replaced",
  unixOnly,
  (t) => {
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
  },
)

test("a config that does not parse is reported, not overwritten", unixOnly, () => {
  const data = temporaryRoot()
  mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
  const path = join(data, "dev/config.json")
  writeFileSync(path, "{ not json", { mode: 0o600 })
  const output = run(data)
  assert.match(output, /not valid JSON/)
  assert.equal(readFileSync(path, "utf8"), "{ not json")
})

test(
  "a clone with no harness installed is told what to run, and writes nothing",
  unixOnly,
  () => {
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
    const output = execFileSync(
      "node",
      [join(checkout, "scripts/dev-agent-config.mjs")],
      {
        encoding: "utf8",
        env: { ...process.env, NESSA_DATA_DIR: data, NESSA_STAGE: "dev" },
      },
    )
    assert.match(output, /harness is not installed/)
    assert.match(output, /npm ci --omit=dev/)
    assert.equal(existsSync(join(data, "dev/config.json")), false)
  },
)

test("a config that says agent is null is an answer, not a gap to fill", unixOnly, () => {
  // The server reads `agent` as an Option, so null is a gateway with no agent
  // — a configuration that starts. Before, it reached `agent.node` and took
  // `pnpm server:run` down with it, because the two are chained with `&&`.
  const data = temporaryRoot()
  mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
  const path = join(data, "dev/config.json")
  writeFileSync(path, `{"agent": null}`, { mode: 0o600 })

  const output = run(data)

  assert.match(output, /"agent": null/)
  assert.match(output, /left as it is/)
  assert.equal(readFileSync(path, "utf8"), `{"agent": null}`)
})

/**
 * The window between reading the configuration and writing it back is real:
 * locating a node, asking cargo about the MCP binary, making the workspace.
 * Another writer in that window — a second checkout's dev loop, an editor
 * saving settings — must not have their work renamed away by this one.
 */
test(
  "a writer during the run keeps their settings, and their agent",
  unixOnly,
  async (t) => {
    const { publish } = await import("./dev-agent-config.mjs")
    const data = temporaryRoot()
    mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
    const path = join(data, "dev/config.json")
    const agent = { node: "/usr/bin/node", acpEntry: "/checkout/dist/index.js" }

    // A setting written after this run read the file, and before it writes.
    writeFileSync(path, JSON.stringify({ session: { writeTimeoutMs: 75 } }), {
      mode: 0o600,
    })
    const wrote = publish({
      configPath: path,
      agent,
      acpEntry: agent.acpEntry,
      node: agent.node,
      interrupt: () =>
        writeFileSync(path, JSON.stringify({ session: { writeTimeoutMs: 321 } }), {
          mode: 0o600,
        }),
    })

    assert.equal(wrote, true)
    const saved = JSON.parse(readFileSync(path, "utf8"))
    assert.equal(saved.session.writeTimeoutMs, 321, "the newer setting survives")
    assert.deepEqual(saved.agent, agent)

    // And an agent that arrived in that window is theirs, not this run's.
    writeFileSync(path, JSON.stringify({}), { mode: 0o600 })
    const theirs = { node: "/their/node", acpEntry: "/their/entry.js" }
    const second = publish({
      configPath: path,
      agent,
      acpEntry: agent.acpEntry,
      node: agent.node,
      interrupt: () =>
        writeFileSync(path, JSON.stringify({ agent: theirs }), { mode: 0o600 }),
    })

    assert.equal(second, false, "nothing was written over them")
    assert.deepEqual(JSON.parse(readFileSync(path, "utf8")).agent, theirs)
  },
)

/**
 * Two runs configuring at once. The window the earlier `interrupt` test could
 * not reach is the one after the final read: both runs decide to write, and the
 * second rename replaces the first — atomic, and still a lost update. The lock
 * is what makes the second stand down instead.
 */
test(
  "a second run standing on the same file leaves it to the first",
  unixOnly,
  async () => {
    const { publish } = await import("./dev-agent-config.mjs")
    const data = temporaryRoot()
    mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
    const path = join(data, "dev/config.json")
    writeFileSync(path, JSON.stringify({}), { mode: 0o600 })

    const mine = { node: "/usr/bin/node", acpEntry: "/mine/index.js" }
    const theirs = { node: "/usr/bin/node", acpEntry: "/theirs/index.js" }
    let second

    // The other run happens while this one holds the lock, which is exactly the
    // interleaving that used to lose a write.
    const first = publish({
      configPath: path,
      agent: mine,
      acpEntry: mine.acpEntry,
      node: mine.node,
      interrupt: () => {
        second = publish({
          configPath: path,
          agent: theirs,
          acpEntry: theirs.acpEntry,
          node: theirs.node,
        })
      },
    })

    assert.equal(first, true, "the run holding the lock writes")
    assert.equal(second, false, "the run that could not take it stands down")
    assert.deepEqual(JSON.parse(readFileSync(path, "utf8")).agent, mine)
  },
)

/** A run killed before releasing must not block the next one for ever. */
test("a lock whose owner is gone is taken rather than obeyed", unixOnly, async () => {
  const { publish } = await import("./dev-agent-config.mjs")
  const data = temporaryRoot()
  mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
  const path = join(data, "dev/config.json")
  writeFileSync(path, JSON.stringify({}), { mode: 0o600 })
  // A pid that cannot be running: process 0 is not a user process, and the
  // holder file is what a killed run leaves behind.
  writeFileSync(`${path}.lock`, "2147483647 2026-01-01T00:00:00.000Z\n", { mode: 0o600 })

  const agent = { node: "/usr/bin/node", acpEntry: "/mine/index.js" }
  const wrote = publish({
    configPath: path,
    agent,
    acpEntry: agent.acpEntry,
    node: agent.node,
  })

  assert.equal(wrote, true)
  assert.deepEqual(JSON.parse(readFileSync(path, "utf8")).agent, agent)
  assert.equal(existsSync(`${path}.lock`), false, "the lock is released")
})
