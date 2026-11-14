import { strict as assert } from "node:assert"
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { DatabaseSync } from "node:sqlite"
import { test } from "node:test"

import {
  asEarlierPlaces,
  asOlderBuild,
  metadataFile,
  placeOf,
  schemaVersion,
} from "./stress-upgrade.mjs"

const definition = readFileSync(
  new URL(
    "../../../../crates/nessa-server/src/conversation/infrastructure/schema.sql",
    import.meta.url,
  ),
  "utf8",
)

/** A current metadata file, as the gateway creates it, under a data directory. */
function current(fill = "") {
  const directory = mkdtempSync(join(tmpdir(), "stress-upgrade-"))
  const conversations = join(directory, "ci", "instances", "x", "conversations")
  mkdirSync(conversations, { recursive: true })
  const path = join(conversations, "metadata.sqlite3")
  const database = new DatabaseSync(path)
  database.exec(definition)
  database.exec(`INSERT INTO catalogue_owners VALUES ('org', 'alice', 1);
    INSERT INTO conversations VALUES ('c1', 'org', 'alice', 'panel', 'create', 1, 'claude',
      'm', 'ask', 1, 1);
    ${fill}`)
  database.close()
  return { directory, path }
}

const tables = (path) => {
  const database = new DatabaseSync(path, { readOnly: true })
  const names = database
    .prepare("SELECT name FROM sqlite_schema WHERE type = 'table'")
    .all()
    .map((row) => row.name)
  database.close()
  return names
}

test("a file with nothing in the newer tables goes back to version 6", (t) => {
  const { directory, path } = current()
  t.after(() => rmSync(directory, { recursive: true, force: true }))
  assert.equal(metadataFile(directory), path)
  assert.equal(schemaVersion(path), 10)
  assert.equal(asOlderBuild(path), 6)
  assert.equal(schemaVersion(path), 6)
  assert.ok(!tables(path).includes("closed_threads"))
  assert.ok(!tables(path).includes("agent_switches"))
  assert.ok(!tables(path).includes("unfinished_turns"))
  assert.ok(!tables(path).includes("model_requests"))
  assert.ok(tables(path).includes("conversations"))
})

test("a marked turn keeps its table, at version 7", (t) => {
  const { directory, path } = current(
    "INSERT INTO unfinished_turns (conversation_id) VALUES ('c1');",
  )
  t.after(() => rmSync(directory, { recursive: true, force: true }))
  assert.equal(asOlderBuild(path), 7)
  assert.ok(tables(path).includes("unfinished_turns"))
  assert.ok(!tables(path).includes("model_requests"))
})

test("a model request keeps every table, at version 7", (t) => {
  const { directory, path } =
    current(`INSERT INTO model_requests (conversation_id, request_id,
      initiator, surface, prior_provider, prior_context, requested_provider, requested_context,
      prior_model, prior_mode, requested_model, state, requested_at_ms)
    VALUES ('c1', 'r1', 'alice', 'panel', 'p', 'c', 'p', 'c', 'm', 'ask', 'n', 'pending', 1);`)
  t.after(() => rmSync(directory, { recursive: true, force: true }))
  assert.equal(asOlderBuild(path), 7)
  assert.ok(tables(path).includes("unfinished_turns"))
  assert.ok(tables(path).includes("model_requests"))
})

test("a file with an agent switch goes back to version 9", (t) => {
  const { directory, path } =
    current(`INSERT INTO agent_switches VALUES ('c1', 'r1', 'alice',
    'panel', 'claude', 'm', 'codex', 'n', 'applied', 1);`)
  t.after(() => rmSync(directory, { recursive: true, force: true }))
  assert.equal(asOlderBuild(path), 9)
  assert.equal(schemaVersion(path), 9)
  assert.ok(tables(path).includes("agent_switches"))
  assert.ok(!tables(path).includes("closed_threads"))
})

test("a file with a closed thread is left as the newest build's", (t) => {
  const { directory, path } = current(
    "INSERT INTO closed_threads (conversation_id) VALUES ('c1');",
  )
  t.after(() => rmSync(directory, { recursive: true, force: true }))
  assert.equal(asOlderBuild(path), 10)
  assert.equal(schemaVersion(path), 10)
  assert.ok(tables(path).includes("closed_threads"))
})

test("place records turn back into an older build's placement and binding files", () => {
  const conversations = mkdtempSync(join(tmpdir(), "stress-places-"))
  try {
    const worker = `peer:${"01".repeat(32)}`
    mkdirSync(join(conversations, "places"))
    const put = (id, record) =>
      writeFileSync(
        join(conversations, "places", `${id}.json`),
        `${JSON.stringify({ schemaVersion: 1, ...record })}\n`,
      )
    put("here", { placed: "here" })
    put("host", { placed: "devbox" })
    // Older files the record no longer agrees with are not left to be read.
    mkdirSync(join(conversations, "placements"), { recursive: true })
    mkdirSync(join(conversations, "bindings"), { recursive: true })
    writeFileSync(join(conversations, "placements", "here.json"), "{}")
    writeFileSync(join(conversations, "bindings", "host.json"), "{}")
    put("pool", { placed: "peer:any", worker, context: "c" })
    assert.deepEqual(placeOf(conversations, "pool"), {
      placed: "peer:any",
      worker,
      context: "c",
    })
    assert.deepEqual(asEarlierPlaces(conversations, "pool"), ["pool"])
    assert.deepEqual(placeOf(conversations, "pool"), {
      placed: "peer:any",
      worker,
      context: "c",
    })
    assert.deepEqual(asEarlierPlaces(conversations).sort(), ["here", "host"])
    assert.equal(placeOf(conversations, "here"), null)
    assert.deepEqual(placeOf(conversations, "host"), {
      placed: "devbox",
      worker: undefined,
      context: undefined,
    })
    const file = (directory, id) =>
      JSON.parse(readFileSync(join(conversations, directory, `${id}.json`), "utf8"))
    assert.deepEqual(file("placements", "host"), { schemaVersion: 1, host: "devbox" })
    assert.deepEqual(file("placements", "pool"), { schemaVersion: 1, host: "peer:any" })
    assert.deepEqual(file("bindings", "pool"), { schemaVersion: 1, worker, context: "c" })
    assert.equal(existsSync(join(conversations, "placements", "here.json")), false)
    assert.equal(existsSync(join(conversations, "bindings", "host.json")), false)
    assert.deepEqual(readdirOf(join(conversations, "places")), [])
  } finally {
    rmSync(conversations, { recursive: true, force: true })
  }
})

function readdirOf(directory) {
  return existsSync(directory) ? readdirSync(directory).sort() : []
}
