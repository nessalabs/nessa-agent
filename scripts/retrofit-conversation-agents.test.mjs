import assert from "node:assert/strict"
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { randomUUID } from "node:crypto"
import { test } from "node:test"

import { ORIGINAL_AGENT, plan, retrofit } from "./retrofit-conversation-agents.mjs"

/** A record as the build before this one published it: everything but the agent. */
const before = (id) => ({
  id,
  organization: "org",
  owner: "alice",
  creator_surface: "panel",
  creation_action: "create",
  creation_requested_at_ms: 123,
})

function directoryOf(records) {
  const root = join(tmpdir(), `nessa-retrofit-${randomUUID()}`)
  mkdirSync(root, { recursive: true, mode: 0o700 })
  for (const [name, body] of Object.entries(records))
    writeFileSync(
      join(root, name),
      typeof body === "string" ? body : JSON.stringify(body),
      {
        mode: 0o600,
      },
    )
  return root
}

test("a record written before agents existed is given the only one that could have written it", () => {
  const decision = plan(JSON.stringify(before("c1")))
  assert.equal(decision.state, "retrofit")
  const written = JSON.parse(decision.text)
  assert.equal(written.agent, ORIGINAL_AGENT)
  // Everything else is carried through untouched.
  assert.deepEqual(
    { ...written, agent: undefined },
    { ...before("c1"), agent: undefined },
  )
  // And it reads like a record this build wrote: the agent last, newline ended.
  assert.equal(Object.keys(written).at(-1), "agent")
  assert.ok(decision.text.endsWith("\n"))
})

test("a record that already names its agent is left exactly as it is", () => {
  assert.equal(plan(JSON.stringify({ ...before("c1"), agent: "codex" })).state, "current")
  // Including one naming an agent this build does not know. Somebody's data,
  // and reinterpreting it is the thing this script exists to avoid.
  assert.equal(
    plan(JSON.stringify({ ...before("c1"), agent: "opencode" })).state,
    "current",
  )
})

test("anything that is not a conversation record is skipped, not repaired", () => {
  // The whole risk of a retrofit: a file that happens to be JSON without an
  // `agent` key is not a record, and writing an agent into it is damage.
  for (const [body, why] of [
    ["{not json", /not JSON/],
    ['"a string"', /not a JSON object/],
    ["[]", /not a JSON object/],
    ["null", /not a JSON object/],
    ["{}", /missing id/],
    [JSON.stringify({ id: "c1", owner: "alice" }), /missing organization/],
  ]) {
    const decision = plan(body)
    assert.equal(decision.state, "foreign", body)
    assert.match(decision.why, why)
  }
})

test("a directory is retrofitted once and is a no-op every time after", () => {
  const root = directoryOf({
    "a.json": before("a"),
    "b.json": before("b"),
    "c.json": { ...before("c"), agent: "codex" },
    "junk.json": "{}",
    "notes.txt": "left alone",
  })

  const first = retrofit(root)
  assert.deepEqual(first.retrofitted, ["a.json", "b.json"])
  assert.equal(first.current, 1)
  assert.equal(first.foreign.length, 1)
  assert.equal(first.foreign[0].name, "junk.json")
  assert.match(first.foreign[0].why, /missing id, organization/)
  assert.equal(
    JSON.parse(readFileSync(join(root, "a.json"), "utf8")).agent,
    ORIGINAL_AGENT,
  )
  assert.equal(JSON.parse(readFileSync(join(root, "c.json"), "utf8")).agent, "codex")
  assert.equal(readFileSync(join(root, "notes.txt"), "utf8"), "left alone")

  const second = retrofit(root)
  assert.deepEqual(second.retrofitted, [], "a second run has nothing left to do")
  assert.equal(second.current, 3)

  // No temporary left behind by either run.
  assert.deepEqual(readdirSync(root).sort(), [
    "a.json",
    "b.json",
    "c.json",
    "junk.json",
    "notes.txt",
  ])
})

test("a dry run reports what it would write and writes nothing", () => {
  const root = directoryOf({ "a.json": before("a") })
  const seen = retrofit(root, { dryRun: true })
  assert.deepEqual(seen.retrofitted, ["a.json"])
  assert.equal(
    JSON.parse(readFileSync(join(root, "a.json"), "utf8")).agent,
    undefined,
    "a dry run wrote the record it was only asked about",
  )
})

test("the name written here is the name the server parses", () => {
  // The one cross-language fact this script depends on. It writes a string
  // that `AgentId::parse` has to recognise, and a rename on the Rust side would
  // otherwise turn every retrofitted record into one the server refuses — with
  // every test on both sides still green.
  const source = readFileSync(
    "crates/nessa-server/src/agents/domain/value_objects/agent_id.rs",
    "utf8",
  )
  const claude = source.match(/AgentId::Claude => "([^"]+)"/)
  assert.ok(claude, "AgentId::Claude no longer states its name where this can read it")
  assert.equal(ORIGINAL_AGENT, claude[1])
})
