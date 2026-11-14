/**
 * Turns a stopped gateway's conversation metadata back into the file an
 * older build wrote, so the next start has to bring it forward
 * (`conversation-stress.mjs`, step `upgrade`). Only what the file can lose
 * without losing anything is taken back:
 *
 *   version 6   the build before resumable turns: no `unfinished_turns`
 *               and no `model_requests`, when both are empty
 *   version 7   the build before model changes: no `model_requests`, when
 *               only that is empty
 *   version 7   the build that added `model_requests` without raising the
 *               version, when it holds rows
 *
 *   version 9   the build before closed threads were kept: no
 *               `closed_threads`, when it is empty and a switch is kept
 *
 * Each needs `closed_threads` empty, which version 10 added: a file with a
 * closed thread in it is left as it is, the newest build's. Those before 9
 * also need `agent_switches` empty, which version 9 added.
 *
 * The gateway must be stopped: this writes the file it holds.
 */
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { join, sep } from "node:path"
import { DatabaseSync } from "node:sqlite"

/** The conversation metadata file under a gateway's data directory. */
export function metadataFile(directory) {
  const found = readdirSync(directory, { recursive: true }).filter(
    (each) =>
      String(each).split(sep).slice(-2).join("/") === "conversations/metadata.sqlite3",
  )
  if (found.length !== 1)
    throw new Error(`expected one conversation metadata file, found ${found.length}`)
  return join(directory, String(found[0]))
}

/** The file's `user_version`. */
export function schemaVersion(path) {
  const database = new DatabaseSync(path, { readOnly: true })
  try {
    return database.prepare("PRAGMA user_version").get().user_version
  } finally {
    database.close()
  }
}

/** Rewrites `path` as the oldest build it can be without losing a row; returns that version. */
export function asOlderBuild(path) {
  const database = new DatabaseSync(path)
  try {
    const rows = (table) => database.prepare(`SELECT count(*) AS n FROM ${table}`).get().n
    if (rows("closed_threads") > 0)
      return database.prepare("PRAGMA user_version").get().user_version
    const switches = rows("agent_switches")
    database.exec("BEGIN IMMEDIATE")
    database.exec("DROP TABLE closed_threads")
    if (switches > 0) {
      database.exec("PRAGMA user_version = 9")
      database.exec("COMMIT")
      return 9
    }
    const models = rows("model_requests")
    const turns = rows("unfinished_turns")
    database.exec("DROP TABLE agent_switches")
    if (models === 0) database.exec("DROP TABLE model_requests")
    if (models === 0 && turns === 0) database.exec("DROP TABLE unfinished_turns")
    const version = models === 0 && turns === 0 ? 6 : 7
    database.exec(`PRAGMA user_version = ${version}`)
    database.exec("COMMIT")
    return version
  } finally {
    database.close()
  }
}

/** A JSON file the gateway reads, private as it writes them. */
function writePrivate(directory, name, value) {
  mkdirSync(directory, { recursive: true, mode: 0o700 })
  writeFileSync(join(directory, name), `${JSON.stringify(value)}\n`, { mode: 0o600 })
}

/**
 * Where conversation `id` runs, as the gateway keeps it under its
 * `conversations` directory: `places/<id>.json`, else an older build's
 * `placements/<id>.json` and, for the pool, `bindings/<id>.json`. `null`
 * when none is kept, which is here.
 */
export function placeOf(conversations, id) {
  const read = (directory) => {
    const path = join(conversations, directory, `${id}.json`)
    return existsSync(path) ? JSON.parse(readFileSync(path, "utf8")) : null
  }
  const record = read("places")
  if (record)
    return { placed: record.placed, worker: record.worker, context: record.context }
  const placement = read("placements")
  if (!placement) return null
  const binding = placement.host === "peer:any" ? read("bindings") : null
  return { placed: placement.host, worker: binding?.worker, context: binding?.context }
}

/**
 * Turn the place records under a stopped gateway's `conversations`
 * directory back into the files an older build kept (all of them, or the
 * one of `only`): here as no file at all, anywhere else as
 * `placements/<id>.json` `{schemaVersion, host}`, and a pool conversation's
 * worker as `bindings/<id>.json` `{schemaVersion, worker, context}`. The
 * place records are removed, so the gateway must read the older files.
 * Answers the ids it turned back.
 */
export function asEarlierPlaces(conversations, only) {
  const places = join(conversations, "places")
  if (!existsSync(places)) return []
  const ids = readdirSync(places)
    .filter((name) => name.endsWith(".json"))
    .map((name) => name.slice(0, -".json".length))
    .filter((id) => only === undefined || id === only)
  for (const id of ids) {
    const record = JSON.parse(readFileSync(join(places, `${id}.json`), "utf8"))
    // Each older file says what the record does, or is gone: no placement
    // for here, no binding before a worker holds it.
    if (record.placed !== "here")
      writePrivate(join(conversations, "placements"), `${id}.json`, {
        schemaVersion: 1,
        host: record.placed,
      })
    else rmSync(join(conversations, "placements", `${id}.json`), { force: true })
    if (record.worker)
      writePrivate(join(conversations, "bindings"), `${id}.json`, {
        schemaVersion: 1,
        worker: record.worker,
        ...(record.context ? { context: record.context } : {}),
      })
    else rmSync(join(conversations, "bindings", `${id}.json`), { force: true })
    rmSync(join(places, `${id}.json`))
  }
  return ids
}
