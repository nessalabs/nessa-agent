import assert from "node:assert/strict"
import { randomUUID } from "node:crypto"
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from "node:fs"
import { DatabaseSync } from "node:sqlite"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import { SCHEMA, move } from "./move-conversation-metadata.mjs"

/** Files as the build before this one wrote them. */
const record = (id, extra = {}) => ({
  id,
  organization: "org",
  owner: "alice",
  creator_surface: "panel",
  creation_action: "create",
  creation_requested_at_ms: 123,
  agent: "claude",
  ...extra,
})
const tombstone = (id, extra = {}) => ({
  id,
  organization: "org",
  initiator: "alice",
  surface: "panel",
  request: "delete",
  requested_at_ms: 200,
  provider_session: { state: "recorded", id: "provider-session" },
  provider_erasure: "not_listed",
  erased: false,
  ...extra,
})
const summary = (id, extra = {}) => ({
  id,
  title: "Plan the trip",
  preview: "Plan the trip",
  updated_at_ms: 300,
  archived: true,
  ...extra,
})

/** A `conversations` directory holding `files`, keyed by their path beneath it. */
function conversationsOf(files) {
  const root = join(tmpdir(), `nessa-move-${randomUUID()}`)
  mkdirSync(join(root, "metadata", "deleted"), { recursive: true, mode: 0o700 })
  mkdirSync(join(root, "summaries"), { mode: 0o700 })
  for (const [path, body] of Object.entries(files))
    writeFileSync(
      join(root, path),
      typeof body === "string" ? body : JSON.stringify(body),
      {
        mode: 0o600,
      },
    )
  return root
}
function rows(root, sql) {
  const db = new DatabaseSync(join(root, "metadata.sqlite3"))
  try {
    return db
      .prepare(sql)
      .all()
      .map((row) => ({ ...row }))
  } finally {
    db.close()
  }
}

const one = "0f8fad5b-d9cb-469f-a165-70867728950e"
const two = "7c9e6679-7425-40de-944b-e07fc1f90ae7"

test("M1: every record, tombstone and summary moves into the database, and the files go", async () => {
  const root = conversationsOf({
    [`metadata/${one}.json`]: record(one),
    [`metadata/${two}.json`]: record(two, { agent: "gemini" }),
    [`metadata/deleted/${one}.json`]: tombstone(one),
    [`summaries/${two}.json`]: summary(two),
    // A publish the gateway was stopped in the middle of: never a record.
    "metadata/.nessa-0123456789abcdef0123456789abcdef.tmp": "{",
  })
  const result = await move(root)
  assert.deepEqual(result, {
    state: "moved",
    records: 2,
    tombstones: 1,
    summaries: 1,
    inserted: 4,
  })
  assert.deepEqual(readdirSync(root), ["metadata.sqlite3"])
  assert.equal(statSync(join(root, "metadata.sqlite3")).mode & 0o777, 0o600)
  assert.deepEqual(rows(root, "SELECT * FROM conversations ORDER BY id"), [
    { ...record(one) },
    // An agent this build does not know is somebody's data, carried as it is.
    { ...record(two, { agent: "gemini" }) },
  ])
  assert.deepEqual(rows(root, "SELECT * FROM deletions"), [
    {
      conversation_id: one,
      organization: "org",
      initiator: "alice",
      surface: "panel",
      request: "delete",
      requested_at_ms: 200,
      provider_session: "recorded",
      provider_session_id: "provider-session",
      provider_erasure: "not_listed",
      erased: 0,
    },
  ])
  assert.deepEqual(rows(root, "SELECT * FROM summaries"), [
    {
      conversation_id: two,
      title: "Plan the trip",
      preview: "Plan the trip",
      updated_at_ms: 300,
      archived: 1,
    },
  ])
  // At the version the schema states, which is the one the server opens.
  const stated = Number(
    /^PRAGMA user_version = (\d+);$/m.exec(readFileSync(SCHEMA, "utf8"))[1],
  )
  assert.ok(stated > 0)
  assert.equal(rows(root, "PRAGMA user_version")[0].user_version, stated)
  // M6: nothing left to move.
  assert.deepEqual(await move(root), { state: "nothing" })
})

test("M2, M5: a run cut short after its commit finishes, and a row held differently stops it", async () => {
  const root = conversationsOf({
    [`metadata/${one}.json`]: record(one),
    [`summaries/${one}.json`]: summary(one),
  })
  await move(root)
  // As if the first run had committed and been stopped before removing its
  // files: they are back, and one more beside them.
  mkdirSync(join(root, "metadata"), { mode: 0o700 })
  writeFileSync(join(root, "metadata", `${one}.json`), JSON.stringify(record(one)), {
    mode: 0o600,
  })
  writeFileSync(join(root, "metadata", `${two}.json`), JSON.stringify(record(two)), {
    mode: 0o600,
  })
  assert.deepEqual(await move(root), {
    state: "moved",
    records: 2,
    tombstones: 0,
    summaries: 0,
    inserted: 1,
  })
  assert.deepEqual(readdirSync(root), ["metadata.sqlite3"])
  // Cut short with a record's file gone and its summary's still there: the
  // record is in the database, so the summary is not an orphan.
  mkdirSync(join(root, "summaries"), { mode: 0o700 })
  writeFileSync(join(root, "summaries", `${one}.json`), JSON.stringify(summary(one)), {
    mode: 0o600,
  })
  assert.deepEqual(await move(root), {
    state: "moved",
    records: 0,
    tombstones: 0,
    summaries: 1,
    inserted: 0,
  })
  assert.deepEqual(readdirSync(root), ["metadata.sqlite3"])
  // A file that disagrees with the row already moved is not overwritten, and
  // nothing else in the run is committed.
  mkdirSync(join(root, "metadata"), { mode: 0o700 })
  writeFileSync(
    join(root, "metadata", `${one}.json`),
    JSON.stringify(record(one, { owner: "mallory" })),
    { mode: 0o600 },
  )
  const three = "16fd2706-8baf-433b-82eb-8c7fada847da"
  writeFileSync(join(root, "metadata", `${three}.json`), JSON.stringify(record(three)), {
    mode: 0o600,
  })
  const refused = await move(root)
  assert.equal(refused.state, "refused")
  assert.match(refused.refused[0].why, /already holds conversations/)
  assert.equal(rows(root, "SELECT count(*) AS n FROM conversations")[0].n, 2)
  assert.ok(existsSync(join(root, "metadata", `${three}.json`)))
})

test("M3, M4: a file that cannot be moved is named, and nothing is committed", async () => {
  for (const [path, body, why] of [
    [`metadata/${two}.json`, "{", /not JSON/],
    [
      `metadata/${two}.json`,
      { ...record(two), agent: undefined },
      /retrofit-conversation-agents/,
    ],
    [`metadata/${two}.json`, record(two, { extra: 1 }), /never wrote: extra/],
    [`metadata/${two}.json`, record(one), /not the conversation it is named for/],
    [
      `metadata/${two}.json`,
      record(two, { creation_requested_at_ms: -1 }),
      /creation_requested_at_ms/,
    ],
    ["metadata/notes.txt", "hello", /not a conversation metadata file/],
    [
      `metadata/deleted/${one}.json`,
      tombstone(one, { provider_session: { state: "absent", id: "x" } }),
      /provider_session/,
    ],
    [`metadata/deleted/${two}.json`, tombstone(two), /has no record/],
    [`summaries/${two}.json`, summary(two), /has no record/],
    [`summaries/${one}.json`, summary(one, { archived: "yes" }), /archived/],
  ]) {
    const root = conversationsOf({ [`metadata/${one}.json`]: record(one), [path]: body })
    const result = await move(root)
    assert.equal(result.state, "refused", path)
    assert.equal(result.refused.length, 1, path)
    assert.equal(result.refused[0].path, join(root, path))
    assert.match(result.refused[0].why, why)
    // No database, and every file where it was.
    assert.ok(!existsSync(join(root, "metadata.sqlite3")), path)
    assert.ok(existsSync(join(root, "metadata", `${one}.json`)), path)
    assert.ok(existsSync(join(root, path)), path)
  }
})

test("a dry run reads and reports, and writes nothing", async () => {
  const root = conversationsOf({ [`metadata/${one}.json`]: record(one) })
  assert.deepEqual(await move(root, { dryRun: true }), {
    state: "would move",
    records: 1,
    tombstones: 0,
    summaries: 0,
    inserted: 0,
  })
  assert.ok(!existsSync(join(root, "metadata.sqlite3")))
  assert.ok(existsSync(join(root, "metadata", `${one}.json`)))
})
