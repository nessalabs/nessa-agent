import { strict as assert } from "node:assert"
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { DatabaseSync } from "node:sqlite"
import { test } from "node:test"

import { asOlderBuild, metadataFile, schemaVersion } from "./stress-upgrade.mjs"

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
