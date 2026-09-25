/**
 * Move conversation metadata an earlier build kept as files into the database
 * this build reads.
 *
 * The gateway now keeps ownership records, tombstones and summaries in one
 * SQLite file, `conversations/metadata.sqlite3`, and does not start while
 * `conversations/metadata/` or `conversations/summaries/` is still there: reading both shapes is what CODING_STANDARDS.md's "One
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

/** The longest record, tombstone or summary file the server read. */
const MAX_BYTES = 4096

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

/** A string holding half a character, which the server's parser refused. */
function malformed(value) {
  if (typeof value === "string") return !value.isWellFormed()
  if (fits(value, "object")) return Object.values(value).some(malformed)
  return false
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
export function parse(bytes, fields, id) {
  // What the server read, it read whole and at most this long, as UTF-8.
  if (bytes.length > MAX_BYTES) return { why: `it is longer than ${MAX_BYTES} bytes` }
  let text
  try {
    text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes)
  } catch {
    return { why: "it is not UTF-8" }
  }
  let value
  try {
    value = JSON.parse(text)
  } catch (error) {
    return { why: `it is not JSON (${error.message})` }
  }
  if (!fits(value, "object")) return { why: "it is not a JSON object" }
  // Moved only as the server (compact) or the retrofit (two-space, a final
  // newline) wrote it. That refuses every spelling the server's parser
  // refused — a key twice, a number as `1.0e2`, an escape standing for half a
  // character, a byte-order mark — which taking would move a meaning it never
  // had. It also refuses some the parser read, such as other spacing, which
  // only a hand edit writes; those are named for the operator like the rest.
  const spelled = [JSON.stringify(value), `${JSON.stringify(value, null, 2)}\n`]
  if (!spelled.includes(text) || Object.values(value).some(malformed))
    return { why: "it is not spelled as this server or the retrofit wrote it" }
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
    if (!present(directory)) continue
    // Through a link, the files removed at the end would be another
    // directory's.
    if (!lstatSync(directory).isDirectory()) {
      found.refused.push({ path: directory, why: "it is not a directory of its own" })
      continue
    }
    const exposed = unprivate(lstatSync(directory))
    if (exposed !== undefined) {
      found.refused.push({ path: directory, why: exposed })
      continue
    }
    for (const name of readdirSync(directory).sort()) {
      const path = join(directory, name)
      if (directory === metadata && name === "deleted") continue
      // A temporary is a file the server made; anything else by that name
      // is refused below, before it could stop the removal after a commit.
      if (TEMPORARY.test(name) && lstatSync(path).isFile()) {
        found.temporaries.push(path)
        continue
      }
      const stat = lstatSync(path)
      const exposed = stat.isFile() ? unprivate(stat) : undefined
      if (exposed !== undefined) {
        found.refused.push({ path, why: exposed })
        continue
      }
      if (!stat.isFile() || !name.endsWith(".json")) {
        found.refused.push({
          path,
          why: "it is not conversation metadata; move it out, so this directory can be removed",
        })
        continue
      }
      let text
      try {
        text = readFileSync(path)
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
async function database(conversations, { dryRun = false } = {}) {
  const { DatabaseSync } = await import("node:sqlite")
  const path = join(conversations, DATABASE)
  const definition = readFileSync(SCHEMA, "utf8")
  const version = statedVersion(definition)
  if (dryRun) return inspected(DatabaseSync, path, version)
  let created = false
  if (!present(path)) {
    // Private before SQLite opens it, as the server creates it.
    closeSync(openSync(path, "wx", 0o600))
    created = true
  } else if (!lstatSync(path).isFile()) {
    throw new Error(`${path} is not a regular file`)
  }
  const db = new DatabaseSync(path)
  db.exec(
    "PRAGMA foreign_keys = ON; PRAGMA secure_delete = ON; PRAGMA synchronous = FULL; PRAGMA fullfsync = ON; PRAGMA journal_mode = DELETE;",
  )
  const found = db.prepare("PRAGMA user_version").get().user_version
  if (created || found === 0) {
    const tables = db.prepare("SELECT count(*) AS n FROM sqlite_schema").get().n
    if (tables !== 0) throw new Error(`${path} holds tables with no version`)
    db.exec(`BEGIN; ${definition}`)
    const applied = db.prepare("PRAGMA user_version").get().user_version
    if (applied !== version) {
      db.exec("ROLLBACK")
      throw new Error(`${SCHEMA} states version ${version} and sets ${applied}`)
    }
    db.exec("COMMIT")
  } else if (found !== version) {
    throw new Error(
      `${path} is at schema version ${found}, and this move writes ${version}`,
    )
  }
  return db
}

/**
 * The one version a definition states, as the server's `Schema::new` reads
 * it: exactly one `PRAGMA user_version = N;` line, N above 0.
 */
export function statedVersion(definition) {
  const versions = definition
    .split("\n")
    .map((line) =>
      /^PRAGMA user_version = (.*);$/.exec(
        line.replace(/^[ \t\n\f\r]+|[ \t\n\f\r]+$/g, ""),
      ),
    )
    .filter(Boolean)
  const stated = versions.length === 1 ? versions[0][1] : ""
  const version = /^\d+$/.test(stated) ? Number(stated) : 0
  if (!(version > 0 && version <= 0xffffffff))
    throw new Error(`${SCHEMA} does not state one version above 0`)
  return version
}

/**
 * For a dry run: the database as it is, to check the files against, or null
 * when it holds nothing yet. Nothing is created and no mode is changed; a
 * journal a killed run left is still rolled back, which only a writer can do.
 */
function inspected(DatabaseSync, path, version) {
  if (!present(path)) return null
  if (!lstatSync(path).isFile()) throw new Error(`${path} is not a regular file`)
  const db = new DatabaseSync(path)
  db.exec("PRAGMA foreign_keys = ON;")
  const found = db.prepare("PRAGMA user_version").get().user_version
  const tables = db.prepare("SELECT count(*) AS n FROM sqlite_schema").get().n
  if (found === 0 && tables === 0) {
    db.close()
    return null
  }
  if (found !== version) {
    db.close()
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

/**
 * Why an entry is not private to this user, as the server that wrote it
 * required every file and directory to be (`nessa-local-storage`): someone
 * else's, reachable by others, or a file with a second name. Windows keeps
 * this in ACLs a script cannot read, so there nothing is refused for it.
 */
function unprivate(stat) {
  if (process.platform === "win32") return undefined
  if (stat.uid !== process.getuid())
    return "it is not owned by the user the gateway runs as"
  if ((stat.mode & 0o077) !== 0) return "it is not private to its owner"
  if (stat.isFile() && stat.nlink !== 1) return "it has more than one name"
  return undefined
}

/**
 * Whether `path` is there, told apart from being unable to look: a directory
 * that cannot be read is not one with nothing to move.
 */
function present(path) {
  try {
    lstatSync(path)
    return true
  } catch (error) {
    if (error.code === "ENOENT") return false
    throw error
  }
}

function syncDirectory(directory) {
  // Windows opens no directory for a sync; its moves are written through.
  if (process.platform === "win32") return
  const handle = openSync(directory, "r")
  try {
    fsyncSync(handle)
  } finally {
    closeSync(handle)
  }
}

/** The records a database already there holds. */
async function held(conversations) {
  const path = join(conversations, DATABASE)
  if (!present(path)) return new Set()
  if (!lstatSync(path).isFile()) throw new Error(`${path} is not a regular file`)
  const { DatabaseSync } = await import("node:sqlite")
  // Read and write, never read only: a run killed mid-commit leaves a journal
  // only a writer can roll back, and until it is the file cannot be read.
  const db = new DatabaseSync(path)
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
  if (!present(conversations)) return { state: "nothing" }
  if (!lstatSync(conversations).isDirectory())
    throw new Error(`${conversations} is not a directory`)
  const exposed = unprivate(lstatSync(conversations))
  if (exposed !== undefined) throw new Error(`${conversations}: ${exposed}`)
  if (!present(metadata) && !present(summaries)) return { state: "nothing" }
  const found = read(conversations, await held(conversations))
  if (found.refused.length > 0) return { state: "refused", refused: found.refused }
  const counts = {
    records: found.records.length,
    tombstones: found.tombstones.length,
    summaries: found.summaries.length,
  }
  // A dry run with no database yet has nothing to check the files against.
  const db = await database(conversations, { dryRun })
  // A dry run with no database yet has nothing to check the files against.
  if (db === null)
    return {
      state: "would move",
      ...counts,
      inserted: found.records.length + found.tombstones.length + found.summaries.length,
    }
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
    // A dry run asks everything a real one does, then keeps none of it.
    if (dryRun) {
      db.exec("ROLLBACK")
      return { state: "would move", ...counts, inserted }
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
    if (present(directory)) rmdirSync(directory)
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
