import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import {
  chmodSync,
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

import { agentsBlock, installedAgents, namespaceRoot } from "./dev-agent-config.mjs"

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

/**
 * Run `body` with what the script prints collected into `into`.
 *
 * The script writes to stdout directly rather than through an injected sink,
 * because printing is all it does with these lines; a test that wants to read
 * one takes it from where a person would.
 */
function withStdout(into, body) {
  const write = process.stdout.write.bind(process.stdout)
  process.stdout.write = (chunk) => {
    into.push(String(chunk))
    return true
  }
  try {
    return body()
  } finally {
    process.stdout.write = write
  }
}

/**
 * An agents block that launches Claude from `entry`.
 *
 * The lock tests care about which run's block reaches the file, not what is in
 * one, so they vary the entry script alone and share the rest.
 */
function agentsLaunching(entry) {
  return {
    catalog: "/checkout/models.json",
    workspace: "/data/dev/workspaces/default",
    selected: "claude",
    runtimes: { claude: { command: "/usr/bin/node", args: [entry] } },
  }
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
  const without = agentsBlock({
    checkout: "/checkout",
    namespace: "/data/dev",
    node: "/usr/bin/node",
    mcpBinary: undefined,
    agents: ["claude"],
  })
  for (const key of ["catalog", "workspace"])
    assert.ok(without[key].startsWith("/"), `${key} is absolute`)
  assert.ok(without.runtimes.claude.command.startsWith("/"))
  assert.ok(without.runtimes.claude.args[0].startsWith("/"))
  assert.equal(without.mcpServers, undefined)
  assert.equal(without.workspace, "/data/dev/workspaces/default")
  const with_ = agentsBlock({
    checkout: "/checkout",
    namespace: "/data/dev",
    node: "/usr/bin/node",
    mcpBinary: "/checkout/target/debug/nessa-mcp",
    agents: ["claude"],
  })
  assert.equal(with_.mcpServers[0].command, "/checkout/target/debug/nessa-mcp")
  assert.deepEqual(with_.mcpServers[0].args, [
    "--workspace",
    "/data/dev/workspaces/default",
    "--audit-directory",
    "/data/dev/process-audit",
  ])
})

test("the agent to run on is stated whenever there is a choice to make", unixOnly, () => {
  // The gateway refuses to guess between configured agents, so a block that
  // named two and chose neither would fail every dev launch — and the developer
  // would be answering a question this script never told them it had asked.
  const both = agentsBlock({
    checkout: "/checkout",
    namespace: "/data/dev",
    node: "/usr/bin/node",
    mcpBinary: undefined,
    agents: ["claude", "codex"],
  })
  assert.deepEqual(Object.keys(both.runtimes).sort(), ["claude", "codex"])
  assert.equal(both.selected, "claude")
  assert.equal(both.runtimes.codex.model, "gpt-5.6-terra")
  assert.equal(both.runtimes.claude.model, "claude-sonnet-5")

  // Claude absent, Codex installed: the one that is there is the one chosen.
  const codexOnly = agentsBlock({
    checkout: "/checkout",
    namespace: "/data/dev",
    node: "/usr/bin/node",
    mcpBinary: undefined,
    agents: ["codex"],
  })
  assert.equal(codexOnly.selected, "codex")
})

test("an agent whose harness is not installed is left out entirely", unixOnly, () => {
  // Writing it anyway would name a path that is not there, and the gateway
  // refuses to start on one of those — so a Codex nobody installed would cost
  // the developer their working Claude, not just Codex.
  const entries = []
  const present = installedAgents("/checkout", (path) => {
    entries.push(path)
    return path.includes("claude-acp")
  })
  assert.deepEqual(present, ["claude"])
  assert.ok(entries.some((path) => path.includes("codex-acp")))
})

test(
  "a fresh namespace gets a private config the workspace of which exists",
  unixOnly,
  (t) => {
    const present = installedAgents(root, existsSync)
    if (present.length === 0)
      return t.skip("no ACP harness is installed in this checkout")
    const data = temporaryRoot()
    const output = run(data)
    assert.match(output, /dev agents configured/)
    const path = join(data, "dev/config.json")
    const config = JSON.parse(readFileSync(path, "utf8"))
    // Exactly the agents whose harness is on disk, each launchable as written
    // and running on a model, with the choice between them already made.
    assert.deepEqual(Object.keys(config.agents.runtimes).sort(), [...present].sort())
    for (const [name, runtime] of Object.entries(config.agents.runtimes)) {
      assert.ok(existsSync(runtime.command), `${name} launches something that is there`)
      assert.ok(existsSync(runtime.args[0]), `${name} runs an entry that is there`)
      assert.ok(runtime.model, `${name} names a model`)
    }
    assert.ok(present.includes(config.agents.selected), "the chosen agent is installed")
    assert.ok(existsSync(config.agents.catalog))
    assert.ok(statSync(config.agents.workspace).isDirectory())
    // The gateway refuses to read anything under this root that others can see.
    assert.equal(statSync(path).mode & 0o077, 0)
    assert.equal(statSync(config.agents.workspace).mode & 0o077, 0)

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
    if (installedAgents(root, existsSync).length === 0)
      return t.skip("no ACP harness is installed in this checkout")
    const data = temporaryRoot()
    mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
    const path = join(data, "dev/config.json")
    writeFileSync(path, JSON.stringify({ session: { writeTimeoutMs: 75 } }), {
      mode: 0o600,
    })
    run(data)
    const merged = JSON.parse(readFileSync(path, "utf8"))
    assert.equal(merged.session.writeTimeoutMs, 75)
    assert.ok(merged.agents)

    const mine = {
      agents: {
        catalog: "/nowhere/models.json",
        runtimes: { claude: { command: "/nowhere/node", args: ["/nowhere/index.js"] } },
      },
    }
    writeFileSync(path, JSON.stringify(mine), { mode: 0o600 })
    const output = run(data)
    assert.deepEqual(JSON.parse(readFileSync(path, "utf8")), mine)
    // A hand-written block that cannot launch is reported rather than repaired.
    assert.match(output, /point at files that are not there/)
    assert.match(output, /\/nowhere\/node/)
  },
)

test(
  "a config carrying both the retired key and the new one has the retired one taken out",
  unixOnly,
  () => {
    // What a developer who ran the dev loop on this branch before the retirement
    // landed now has on disk. The `agents` block answers the question this
    // script asks, so it writes no block of its own — and used to stand down
    // saying the file was fine, while `deny_unknown_fields` refused to start
    // the gateway on the `agent` key still sitting beside it.
    //
    // That key is this script's own and nothing reads it, so it is removed
    // rather than reported as a chore. The `agents` answer beside it belongs to
    // whoever wrote it and comes back exactly as it went in.
    const data = temporaryRoot()
    mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
    const path = join(data, "dev/config.json")
    const agents = agentsLaunching("/checkout/dist/index.js")
    writeFileSync(
      path,
      JSON.stringify({
        agent: { node: "/old/node", acpEntry: "/old/entry.js" },
        agents,
        session: { writeTimeoutMs: 75 },
      }),
      { mode: 0o600 },
    )

    const output = run(data)

    assert.match(output, /retired "agent" block/)
    const saved = JSON.parse(readFileSync(path, "utf8"))
    assert.equal(saved.agent, undefined, "the gateway still refuses to start on this")
    assert.deepEqual(saved.agents, agents, "somebody else's answer was rewritten")
    // Only the key this script owned. Another setting in the same file is not
    // its to tidy either.
    assert.equal(saved.session.writeTimeoutMs, 75)

    // And a second run has nothing left to say about it.
    assert.doesNotMatch(run(data), /retired "agent" block/)
  },
)

test(
  "publishing onto that config repairs it in the caller's process rather than ending it",
  unixOnly,
  async () => {
    // `publish` asks the same question again under the lock, and this suite
    // calls it here, in the process running the tests. Whatever that path
    // decides, it must not end the process the way the stand-down path does: a
    // regression there would stop this file at whichever test reached it first
    // and report the tests that did run as a pass.
    const { publish } = await import("./dev-agent-config.mjs")
    const data = temporaryRoot()
    mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
    const path = join(data, "dev/config.json")
    const agents = agentsLaunching("/checkout/dist/index.js")
    writeFileSync(path, JSON.stringify({ agent: { node: "/old/node" }, agents }), {
      mode: 0o600,
    })

    const wrote = publish({
      configPath: path,
      agents,
      node: agents.runtimes.claude.command,
    })

    assert.equal(wrote, false, "somebody else's answer was replaced")
    const saved = JSON.parse(readFileSync(path, "utf8"))
    assert.equal(saved.agent, undefined)
    assert.deepEqual(saved.agents, agents)
  },
)

test(
  "a repair that cannot be written fails the caller instead of reporting success",
  {
    ...unixOnly,
    // Root writes through a read-only directory, so there is no way to make the
    // write fail here. It fails on an ordinary user, which is what CI runs as.
    skip: process.getuid?.() === 0 ? "run as root; cannot make a write fail" : false,
  },
  async () => {
    // The one path left that reports a configuration the gateway will refuse.
    // It throws rather than exiting, for the reason the test above gives.
    const { publish } = await import("./dev-agent-config.mjs")
    const data = temporaryRoot()
    const directory = join(data, "dev")
    mkdirSync(directory, { recursive: true, mode: 0o700 })
    const path = join(directory, "config.json")
    const agents = agentsLaunching("/checkout/dist/index.js")
    writeFileSync(path, JSON.stringify({ agent: { node: "/old/node" }, agents }), {
      mode: 0o600,
    })
    chmodSync(directory, 0o500)

    try {
      assert.throws(
        () => publish({ configPath: path, agents, node: agents.runtimes.claude.command }),
        /could not be repaired/,
      )
    } finally {
      chmodSync(directory, 0o700)
    }
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
    assert.match(output, /no ACP harness is installed/)
    // Every agent it could have configured is named, so the developer knows
    // both are a `npm ci` away rather than only the one this script prefers.
    for (const agent of ["claude-acp", "codex-acp"])
      assert.match(output, new RegExp(`${agent} && npm ci --omit=dev`))
    assert.equal(existsSync(join(data, "dev/config.json")), false)
  },
)

test(
  "a config that says agents is null is an answer, not a gap to fill",
  unixOnly,
  () => {
    // The server reads `agents` as an Option, so null is a gateway with no agents
    // — a configuration that starts. Before, it reached into the block and took
    // `pnpm server:run` down with it, because the two are chained with `&&`.
    const data = temporaryRoot()
    mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
    const path = join(data, "dev/config.json")
    writeFileSync(path, `{"agents": null}`, { mode: 0o600 })

    const output = run(data)

    assert.match(output, /"agents": null/)
    assert.match(output, /left as it is/)
    assert.equal(readFileSync(path, "utf8"), `{"agents": null}`)
  },
)

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
    const agents = agentsLaunching("/checkout/dist/index.js")

    // A setting written after this run read the file, and before it writes.
    writeFileSync(path, JSON.stringify({ session: { writeTimeoutMs: 75 } }), {
      mode: 0o600,
    })
    const wrote = publish({
      configPath: path,
      agents,
      node: agents.runtimes.claude.command,
      interrupt: () =>
        writeFileSync(path, JSON.stringify({ session: { writeTimeoutMs: 321 } }), {
          mode: 0o600,
        }),
    })

    assert.equal(wrote, true)
    const saved = JSON.parse(readFileSync(path, "utf8"))
    assert.equal(saved.session.writeTimeoutMs, 321, "the newer setting survives")
    assert.deepEqual(saved.agents, agents)

    // And an agent that arrived in that window is theirs, not this run's.
    writeFileSync(path, JSON.stringify({}), { mode: 0o600 })
    const theirs = {
      catalog: "/their/models.json",
      runtimes: { claude: { command: "/their/node", args: ["/their/entry.js"] } },
    }
    const second = publish({
      configPath: path,
      agents,
      node: agents.runtimes.claude.command,
      interrupt: () =>
        writeFileSync(path, JSON.stringify({ agents: theirs }), { mode: 0o600 }),
    })

    assert.equal(second, false, "nothing was written over them")
    assert.deepEqual(JSON.parse(readFileSync(path, "utf8")).agents, theirs)
  },
)

/**
 * Every checkout that ran the dev loop before agents became a map has an
 * `agent` block in its `config.json`. The server reads its configuration with
 * `deny_unknown_fields`, so that key left in place is a gateway that will not
 * start — and the run that left it there says it configured the dev agents.
 *
 * This script wrote that block, so retiring it is this script's job: local
 * development data brought to the current shape by the tool that owns it, which
 * is what the standard asks for instead of a reader in the server.
 */
test(
  "the agent block an older version wrote is retired, not left beside",
  unixOnly,
  async () => {
    const { publish } = await import("./dev-agent-config.mjs")
    const data = temporaryRoot()
    mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
    const path = join(data, "dev/config.json")
    const agents = agentsLaunching("/checkout/dist/index.js")

    // Exactly what the previous version of this script left behind, beside a
    // setting that has nothing to do with it.
    writeFileSync(
      path,
      JSON.stringify({
        agent: { node: "/old/node", acpEntry: "/old/entry.js" },
        session: { writeTimeoutMs: 75 },
      }),
      { mode: 0o600 },
    )

    const wrote = publish({
      configPath: path,
      agents,
      node: agents.runtimes.claude.command,
    })

    assert.equal(wrote, true)
    const saved = JSON.parse(readFileSync(path, "utf8"))
    assert.deepEqual(saved.agents, agents)
    assert.equal(saved.agent, undefined, "the old block would refuse the gateway")
    assert.deepEqual(Object.keys(saved).sort(), ["agents", "session"])
    // Only the key this script owned. Somebody else's settings are not its to tidy.
    assert.equal(saved.session.writeTimeoutMs, 75)
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

    const mine = agentsLaunching("/mine/index.js")
    const theirs = agentsLaunching("/theirs/index.js")
    let second

    // The other run happens while this one holds the lock, which is exactly the
    // interleaving that used to lose a write.
    const first = publish({
      configPath: path,
      agents: mine,
      node: mine.runtimes.claude.command,
      interrupt: () => {
        second = publish({
          configPath: path,
          agents: theirs,
          node: theirs.runtimes.claude.command,
        })
      },
    })

    assert.equal(first, true, "the run holding the lock writes")
    assert.equal(second, false, "the run that could not take it stands down")
    assert.deepEqual(JSON.parse(readFileSync(path, "utf8")).agents, mine)
  },
)

/**
 * A lock left by a killed run is reported, not taken.
 *
 * Taking it is the race the lock exists to prevent: between reading the dead
 * pid and unlinking the file, that file can become a live run's lock, and two
 * runs then write the same configuration. So the file is left where it is and
 * the person is told the command that clears it.
 */
test("a lock whose owner is gone is reported, not taken", unixOnly, async () => {
  const { publish } = await import("./dev-agent-config.mjs")
  const data = temporaryRoot()
  mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
  const path = join(data, "dev/config.json")
  writeFileSync(path, JSON.stringify({}), { mode: 0o600 })
  // A pid that cannot be running, which is what a killed run leaves behind.
  const holder = "2147483647 stale-token 2026-01-01T00:00:00.000Z\n"
  writeFileSync(`${path}.lock`, holder, { mode: 0o600 })

  const agents = agentsLaunching("/mine/index.js")
  const said = []
  const wrote = withStdout(said, () =>
    publish({
      configPath: path,
      agents,
      node: agents.runtimes.claude.command,
    }),
  )

  assert.equal(wrote, false, "the run stands down")
  assert.deepEqual(JSON.parse(readFileSync(path, "utf8")), {}, "nothing is written")
  assert.equal(
    readFileSync(`${path}.lock`, "utf8"),
    holder,
    "the lock it did not take is left exactly as it was",
  )
  assert.match(said.join("\n"), /rm .*config\.json\.lock/, "says how to clear it")
})

/**
 * Two runs meeting the same stale lock is what made stealing unsafe: both read
 * the dead pid, both delete it, and both then believe they hold the lock. The
 * regression is that neither of them writes.
 */
test("two runs finding the same stale lock do not both write", unixOnly, async () => {
  const { publish } = await import("./dev-agent-config.mjs")
  const data = temporaryRoot()
  mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
  const path = join(data, "dev/config.json")
  writeFileSync(path, JSON.stringify({}), { mode: 0o600 })
  const holder = "2147483647 stale-token 2026-01-01T00:00:00.000Z\n"
  writeFileSync(`${path}.lock`, holder, { mode: 0o600 })

  const attempt = (entry) => {
    const agents = agentsLaunching(entry)
    return publish({
      configPath: path,
      agents,
      node: agents.runtimes.claude.command,
    })
  }

  const said = []
  const [first, second] = withStdout(said, () => [
    attempt("/mine/index.js"),
    attempt("/theirs/index.js"),
  ])

  assert.equal(first, false, "neither takes a lock it cannot own")
  assert.equal(second, false)
  assert.deepEqual(JSON.parse(readFileSync(path, "utf8")), {}, "neither writes")
  assert.equal(readFileSync(`${path}.lock`, "utf8"), holder, "the lock is left alone")
})

/** A lock cleared by hand and retaken belongs to whoever has it now. */
test("releasing does not remove somebody else's lock", unixOnly, async () => {
  const { publish } = await import("./dev-agent-config.mjs")
  const data = temporaryRoot()
  mkdirSync(join(data, "dev"), { recursive: true, mode: 0o700 })
  const path = join(data, "dev/config.json")
  writeFileSync(path, JSON.stringify({}), { mode: 0o600 })

  const agents = agentsLaunching("/mine/index.js")
  const theirs = `${process.pid + 1} their-token 2026-01-01T00:00:00.000Z\n`

  const wrote = publish({
    configPath: path,
    agents,
    node: agents.runtimes.claude.command,
    // Somebody clears the lock by hand and another run takes it, while this run
    // is between acquiring and writing.
    interrupt: () => writeFileSync(`${path}.lock`, theirs, { mode: 0o600 }),
  })

  assert.equal(wrote, true)
  assert.equal(
    readFileSync(`${path}.lock`, "utf8"),
    theirs,
    "the other run is still holding its lock",
  )
})
