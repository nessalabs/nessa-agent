/**
 * Move conversation metadata an earlier build kept as files into the database
 * this build reads.
 *
 * The gateway now keeps ownership records, tombstones and summaries in one
 * SQLite file, `conversations/metadata.sqlite3`, and refuses to start its
 * conversations while `conversations/metadata/` or `conversations/summaries/`
 * is still there: reading both shapes is what CODING_STANDARDS.md's "One
 * current contract" forbids. This is the move, run once, with the gateway
 * stopped. See docs/adr/todo/196-conversation-metadata-database.md, whose
 * table M1–M7 this follows row for row.
 *
 * Nothing is committed unless every file can be moved (M3, M4): each one that
 * cannot is named, and the exit is nonzero, so the operator repairs it or moves
 * it aside and runs this again. Once committed, the files are removed, and a
 * run interrupted while removing them finishes the next time (M2, M5). Records
 * from before records named their agent are refused with the retrofit's name:
 * run `scripts/retrofit-conversation-agents.mjs` first.
 *
 * The file shapes read here are the ones the build before this one wrote. This
 * script is a mover for this one change, deleted together with the gateway's
 * refusal once no namespace predates it.
 *
 * Usage:
 *
 *   node scripts/move-conversation-metadata.mjs [--dry-run] [directory]
 *
 * With no directory it moves this machine's own namespace's `conversations`
 * directory, resolved exactly as the dev loop resolves it, including
 * NESSA_STAGE, NESSA_INSTANCE and NESSA_DATA_DIR.
 */

import {
  closeSync,
  existsSync,
  fsyncSync,
  lstatSync,
  openSync,
  readFileSync,
  readdirSync,
  rmdirSync,
  unlinkSync,
} from "node:fs"
import { realpathSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

import { namespaceRoot } from "./dev-agent-config.mjs"

const here = dirname(fileURLToPath(import.meta.url))

/** The one definition of the tables and their version, which the server includes too. */
export const SCHEMA = join(
  here,
  "..",
  "crates",
  "nessa-server",
  "src",
  "conversation",
  "infrastructure",
  "schema.sql",
)

/** The name the server gives its private temporaries, which a publish in progress left. */
const TEMPORARY = /^\.nessa-[0-9a-f]{32}\.tmp$/

const DATABASE = "metadata.sqlite3"

/** Fields each file holds, exactly: the build before this one refused any other. */
const RECORD = {
  id: "string",
  organization: "string",
  owner: "string",
  creator_surface: "string",
  creation_action: "string",
  creation_requested_at_ms: "integer",
  agent: "string",
}
const TOMBSTONE = {
  id: "string",
  organization: "string",
  initiator: "string",
  surface: "string",
  request: "string",
  requested_at_ms: "integer",
  provider_session: "object",
  provider_erasure: "string?",
  erased: "boolean",
}
const SUMMARY = {
  id: "string",
  title: "string?",
  preview: "string?",
  updated_at_ms: "integer",
  archived: "boolean",
}

function fits(value, type) {
  if (type.endsWith("?")) return value === null || fits(value, type.slice(0, -1))
  if (type === "integer") return Number.isSafeInteger(value) && value >= 0
  if (type === "object")
    return value !== null && typeof value === "object" && !Array.isArray(value)
  return typeof value === type
}

/**
 * What one file holds, as the row it becomes, or why it cannot be moved.
 *
 * Its shape only: which fields, of which types, naming the conversation the
 * file is named for. Whether a value is valid — an identity, a title's length
 * — is the server's to decide when it reads the row, as it decided when it
 * read the file; a row it cannot read is refused where it is read.
 *
 * @returns {{row: object} | {why: string}}
 */
export function parse(text, fields, id) {
  let value
  try {
    value = JSON.parse(text)
  } catch (error) {
    return { why: `it is not JSON (${error.message})` }
  }
  if (!fits(value, "object")) return { why: "it is not a JSON object" }
  if (fields === RECORD && value.agent === undefined)
    return {
      why: "it names no agent; run node scripts/retrofit-conversation-agents.mjs first",
    }
  const unknown = Object.keys(value).filter((key) => !Object.hasOwn(fields, key))
  if (unknown.length > 0)
    return { why: `it has fields this build never wrote: ${unknown.join(", ")}` }
  for (const [field, type] of Object.entries(fields))
    if (!Object.hasOwn(value, field) || !fits(value[field], type))
      return { why: `its ${field} is missing or not ${type.replace("?", " or null")}` }
  if (value.id !== id)
    return {
      why: `it names ${JSON.stringify(value.id)}, not the conversation it is named for`,
    }
  return { row: value }
}

/** A tombstone's provider session, as its two columns. */
function providerSession(session) {
  const { state, id, ...rest } = session
  if (Object.keys(rest).length > 0) return undefined
  if (state === "recorded") return typeof id === "string" ? [state, id] : undefined
  if (["unread", "absent", "unknown"].includes(state) && id === undefined)
    return [state, null]
  return undefined
}

/**
 * Every file under `conversations`, sorted into rows, and those that cannot be
 * moved. `held` names the records the database already holds, which a
 * tombstone or summary may name as well as a record's file.
 */
export function read(conversations, held = new Set()) {
  const metadata = join(conversations, "metadata")
  const deleted = join(metadata, "deleted")
  const summaries = join(conversations, "summaries")
  const found = {
    records: [],
    tombstones: [],
    summaries: [],
    refused: [],
    temporaries: [],
  }
  const directories = [
    [metadata, RECORD, found.records],
    [deleted, TOMBSTONE, found.tombstones],
    [summaries, SUMMARY, found.summaries],
  ]
  for (const [directory, fields, rows] of directories) {
    if (!existsSync(directory)) continue
    for (const name of readdirSync(directory).sort()) {
      const path = join(directory, name)
      if (directory === metadata && name === "deleted") continue
      if (TEMPORARY.test(name)) {
        found.temporaries.push(path)
        continue
      }
      const stat = lstatSync(path)
      if (!stat.isFile() || !name.endsWith(".json")) {
        found.refused.push({ path, why: "it is not a conversation metadata file" })
        continue
      }
      let text
      try {
        text = readFileSync(path, "utf8")
      } catch (error) {
        found.refused.push({ path, why: `it could not be read (${error.message})` })
        continue
      }
      const parsed = parse(text, fields, name.slice(0, -".json".length))
      if (parsed.why !== undefined) {
        found.refused.push({ path, why: parsed.why })
        continue
      }
      if (fields === TOMBSTONE) {
        const session = providerSession(parsed.row.provider_session)
        if (session === undefined) {
          found.refused.push({
            path,
            why: "its provider_session is not one this build wrote",
          })
          continue
        }
        parsed.row.provider_session = session
      }
      rows.push({ path, row: parsed.row })
    }
  }
  // A tombstone or summary names its record by a foreign key (M4): a record
  // moved already by a run cut short counts as much as one still a file (M5).
  const recorded = new Set([...held, ...found.records.map(({ row }) => row.id)])
  for (const rows of [found.tombstones, found.summaries])
    for (let index = rows.length - 1; index >= 0; index -= 1)
      if (!recorded.has(rows[index].row.id)) {
        found.refused.push({
          path: rows[index].path,
          why: "its conversation has no record",
        })
        rows.splice(index, 1)
      }
  return found
}

/** The columns each table's row is written with, in order. */
const COLUMNS = {
  conversations: {
    names: [
      "id",
      "organization",
      "owner",
      "creator_surface",
      "creation_action",
      "creation_requested_at_ms",
      "agent",
    ],
    values: (row) => [
      row.id,
      row.organization,
      row.owner,
      row.creator_surface,
      row.creation_action,
      row.creation_requested_at_ms,
      row.agent,
    ],
    key: "id",
  },
  deletions: {
    names: [
      "conversation_id",
      "organization",
      "initiator",
      "surface",
      "request",
      "requested_at_ms",
      "provider_session",
      "provider_session_id",
      "provider_erasure",
      "erased",
    ],
    values: (row) => [
      row.id,
      row.organization,
      row.initiator,
      row.surface,
      row.request,
      row.requested_at_ms,
      row.provider_session[0],
      row.provider_session[1],
      row.provider_erasure,
      row.erased ? 1 : 0,
    ],
    key: "conversation_id",
  },
  summaries: {
    names: ["conversation_id", "title", "preview", "updated_at_ms", "archived"],
    values: (row) => [
      row.id,
      row.title,
      row.preview,
      row.updated_at_ms,
      row.archived ? 1 : 0,
    ],
    key: "conversation_id",
  },
}

/**
 * The database beside the old directories: created privately from the schema
 * when absent, refused when it is at another version.
 */
async function database(conversations) {
  const { DatabaseSync } = await import("node:sqlite")
  const path = join(conversations, DATABASE)
  const definition = readFileSync(SCHEMA, "utf8")
  const version = Number(/^PRAGMA user_version = (\d+);$/m.exec(definition)?.[1])
  let created = false
  if (!existsSync(path)) {
    // Private before SQLite opens it, as the server creates it.
    closeSync(openSync(path, "wx", 0o600))
    created = true
  } else if (!lstatSync(path).isFile()) {
    throw new Error(`${path} is not a regular file`)
  }
  const db = new DatabaseSync(path)
  db.exec(
    "PRAGMA foreign_keys = ON; PRAGMA secure_delete = ON; PRAGMA synchronous = FULL; PRAGMA journal_mode = DELETE;",
  )
  const found = db.prepare("PRAGMA user_version").get().user_version
  if (created || found === 0) {
    const tables = db.prepare("SELECT count(*) AS n FROM sqlite_schema").get().n
    if (tables !== 0) throw new Error(`${path} holds tables with no version`)
    db.exec(`BEGIN; ${definition} COMMIT;`)
  } else if (found !== version) {
    throw new Error(
      `${path} is at schema version ${found}, and this move writes ${version}`,
    )
  }
  return db
}

/** Insert each row the database does not hold; name each one it holds differently (M2). */
function insert(db, table, rows) {
  const { names, values, key } = COLUMNS[table]
  const select = db.prepare(`SELECT ${names.join(", ")} FROM ${table} WHERE ${key} = ?`)
  const write = db.prepare(
    `INSERT INTO ${table} (${names.join(", ")}) VALUES (${names.map(() => "?").join(", ")})`,
  )
  const conflicts = []
  let inserted = 0
  for (const { path, row } of rows) {
    const wanted = values(row)
    const held = select.get(row.id)
    if (held === undefined) {
      write.run(...wanted)
      inserted += 1
    } else if (names.some((name, index) => held[name] !== wanted[index])) {
      conflicts.push({
        path,
        why: `the database already holds ${table} ${row.id} differently`,
      })
    }
  }
  return { inserted, conflicts }
}

function syncDirectory(directory) {
  const handle = openSync(directory, "r")
  try {
    fsyncSync(handle)
  } finally {
    closeSync(handle)
  }
}

/** The records a database already there holds, read without writing to it. */
async function held(conversations) {
  const path = join(conversations, DATABASE)
  if (!existsSync(path)) return new Set()
  const { DatabaseSync } = await import("node:sqlite")
  const db = new DatabaseSync(path, { readOnly: true })
  try {
    const table = db
      .prepare("SELECT count(*) AS n FROM sqlite_schema WHERE name = 'conversations'")
      .get().n
    if (table === 0) return new Set()
    return new Set(
      db
        .prepare("SELECT id FROM conversations")
        .all()
        .map(({ id }) => id),
    )
  } finally {
    db.close()
  }
}

/**
 * Move everything under `conversations`.
 *
 * @returns {Promise<{state: "nothing"} | {state: "refused", refused: Array<{path: string, why: string}>} | {state: "moved" | "would move", records: number, tombstones: number, summaries: number, inserted: number}>}
 */
export async function move(conversations, { dryRun = false } = {}) {
  const metadata = join(conversations, "metadata")
  const summaries = join(conversations, "summaries")
  if (!existsSync(metadata) && !existsSync(summaries)) return { state: "nothing" }
  const found = read(conversations, await held(conversations))
  if (found.refused.length > 0) return { state: "refused", refused: found.refused }
  const counts = {
    records: found.records.length,
    tombstones: found.tombstones.length,
    summaries: found.summaries.length,
  }
  if (dryRun) return { state: "would move", ...counts, inserted: 0 }
  const db = await database(conversations)
  let inserted = 0
  try {
    db.exec("BEGIN IMMEDIATE")
    const conflicts = []
    for (const [table, rows] of [
      ["conversations", found.records],
      ["deletions", found.tombstones],
      ["summaries", found.summaries],
    ]) {
      const result = insert(db, table, rows)
      inserted += result.inserted
      conflicts.push(...result.conflicts)
    }
    if (conflicts.length > 0) {
      db.exec("ROLLBACK")
      return { state: "refused", refused: conflicts }
    }
    db.exec("COMMIT")
  } catch (error) {
    if (db.isTransaction) db.exec("ROLLBACK")
    throw error
  } finally {
    db.close()
  }
  syncDirectory(conversations)
  // Committed: only now do the files go.
  for (const { path } of [...found.records, ...found.tombstones, ...found.summaries])
    unlinkSync(path)
  for (const path of found.temporaries) unlinkSync(path)
  for (const directory of [join(metadata, "deleted"), metadata, summaries])
    if (existsSync(directory)) rmdirSync(directory)
  syncDirectory(conversations)
  return { state: "moved", ...counts, inserted }
}

const USAGE = "usage: node scripts/move-conversation-metadata.mjs [--dry-run] [directory]"

async function main() {
  const args = process.argv.slice(2)
  const dryRun = args.includes("--dry-run")
  const given = args.filter((argument) => argument !== "--dry-run")
  if (given.includes("--help") || given.includes("-h")) {
    console.log(USAGE)
    return
  }
  const unknown = given.find((argument) => argument.startsWith("-"))
  if (unknown !== undefined) {
    console.error(`unknown option: ${unknown}`)
    console.error(USAGE)
    process.exit(2)
  }
  if (given.length > 1) {
    console.error(USAGE)
    process.exit(2)
  }
  const conversations = given[0] ?? join(namespaceRoot(), "conversations")
  let result
  try {
    result = await move(conversations, { dryRun })
  } catch (error) {
    console.error(`→ could not move ${conversations}: ${error.message}`)
    if (error.code === "EACCES" || error.code === "EPERM")
      console.error("  this metadata is private to the user the gateway runs as")
    process.exit(1)
  }
  if (result.state === "nothing") {
    console.log(`→ no conversation metadata files at ${conversations}; nothing to move`)
    return
  }
  if (result.state === "refused") {
    console.error(
      `→ nothing was moved from ${conversations}; each file below needs repairing or moving aside first:`,
    )
    for (const { path, why } of result.refused) console.error(`    ${path}: ${why}`)
    process.exit(1)
  }
  console.log(`→ ${conversations}`)
  console.log(
    `    ${result.state} ${result.records} records, ${result.tombstones} tombstones and ${result.summaries} summaries into ${DATABASE}`,
  )
  if (dryRun) console.log("  rerun without --dry-run to move them")
}

if (
  process.argv[1] !== undefined &&
  realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)
)
  await main()
