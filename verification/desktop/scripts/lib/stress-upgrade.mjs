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
import { readdirSync } from "node:fs"
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
