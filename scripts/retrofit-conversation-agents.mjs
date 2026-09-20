/**
 * Give conversation records written before this build the agent they ran on.
 *
 * A conversation record now states its agent, and the server refuses one that
 * does not rather than reading it as Claude's — a reader for data written by an
 * older build is what CODING_STANDARDS.md's "One current contract" forbids. The
 * same rule asks for the data to be brought to the current shape instead, and
 * this is the tool that does it. Run once, after upgrading; it is a no-op every
 * time after that.
 *
 * Naming Claude is honest rather than a guess. A record without the field was
 * published while Claude was the only agent this server could start, so Claude
 * is the agent whose transcript and provider session that conversation holds.
 * Nothing else could have written it.
 *
 * Usage:
 *
 *   node scripts/retrofit-conversation-agents.mjs [--dry-run] [directory]
 *
 * With no directory it retrofits this machine's own namespace, resolved exactly
 * as the dev loop resolves it, including NESSA_STAGE, NESSA_INSTANCE and
 * NESSA_DATA_DIR. The gateway must not be running: it holds these records.
 */

import { readdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } from "node:fs"
import { randomUUID } from "node:crypto"
import { join } from "node:path"
import { fileURLToPath } from "node:url"
import { realpathSync } from "node:fs"

import { namespaceRoot } from "./dev-agent-config.mjs"

/** The agent every record that names none was created on. */
export const ORIGINAL_AGENT = "claude"

/**
 * Every field a current record states besides the agent.
 *
 * Named here rather than inferred, because this script decides whether a file
 * is a conversation record at all, and a file that merely happens to be JSON
 * without an `agent` key is not one. Getting that wrong writes an agent into
 * somebody else's file.
 */
const REQUIRED = [
  "id",
  "organization",
  "owner",
  "creator_surface",
  "creation_action",
  "creation_requested_at_ms",
]

/**
 * What one record needs, read from its bytes.
 *
 * `current` already names an agent and is left exactly as it is — including a
 * record naming an agent this build does not know, which is somebody's data and
 * not this script's to reinterpret. `foreign` is anything that is not a
 * conversation record, which is reported and skipped rather than repaired.
 *
 * @returns {{state: "current"} | {state: "retrofit", text: string} | {state: "foreign", why: string}}
 */
export function plan(text, agent = ORIGINAL_AGENT) {
  let record
  try {
    record = JSON.parse(text)
  } catch (error) {
    return { state: "foreign", why: `it is not JSON (${error.message})` }
  }
  if (record === null || typeof record !== "object" || Array.isArray(record))
    return { state: "foreign", why: "it is not a JSON object" }
  if (record.agent !== undefined) return { state: "current" }
  const missing = REQUIRED.filter((field) => record[field] === undefined)
  if (missing.length > 0)
    return { state: "foreign", why: `it is missing ${missing.join(", ")}` }
  // The agent goes last, which is where a record written by this build carries
  // it, so a retrofitted record and a fresh one read the same.
  return { state: "retrofit", text: `${JSON.stringify({ ...record, agent }, null, 2)}\n` }
}

/**
 * Replace a record's bytes without ever leaving a half-written one in its place.
 *
 * Written through to whatever the name really points at. A rename onto a
 * symlink replaces the link with a regular file and leaves the record it
 * pointed at untouched — still without an agent, and now unreachable by the
 * name the gateway knows it by. The server's own writer never does that, so
 * neither does this.
 */
function rewrite(path, text) {
  // The temp keeps the name it was reached by and only the rename follows the
  // link. Putting it beside the target instead would leave a run interrupted
  // mid-write with a `.tmp` outside the metadata directory, which is the one
  // place the server's `PrivateTempFile::clear_stale` looks.
  const temporary = `${path}.${randomUUID()}.tmp`
  const target = realpathSync(path)
  try {
    writeFileSync(temporary, text, { mode: 0o600, flag: "wx" })
    renameSync(temporary, target)
  } catch (error) {
    try {
      unlinkSync(temporary)
    } catch {
      // Nothing to clean up.
    }
    throw error
  }
}

/**
 * Retrofit every record in one metadata directory.
 *
 * One entry that cannot be read or written does not end the run. It used to:
 * the throw escaped with the whole accumulated result inside it, so a
 * directory that was already half converted was reported as a failure of the
 * lot, with nothing saying which half — and nothing saying which file was the
 * obstruction either. A subdirectory named `something.json`, or one record
 * owned by another user, was enough. Such an entry joins `failed` beside
 * `foreign` now, the rest are converted, and the caller exits nonzero knowing
 * both what was done and what to fix. A second run is a no-op on everything
 * already converted, so finishing is always safe.
 *
 * @returns {{retrofitted: string[], current: number, foreign: Array<{name: string, why: string}>, failed: Array<{name: string, why: string}>}}
 */
export function retrofit(directory, { dryRun = false } = {}) {
  const names = readdirSync(directory).filter((name) => name.endsWith(".json"))
  const result = { retrofitted: [], current: 0, foreign: [], failed: [] }
  for (const name of names.sort()) {
    const path = join(directory, name)
    let decision
    try {
      decision = plan(readFileSync(path, "utf8"))
    } catch (error) {
      result.failed.push({ name, why: `it could not be read (${error.message})` })
      continue
    }
    if (decision.state === "current") result.current += 1
    else if (decision.state === "foreign")
      result.foreign.push({ name, why: decision.why })
    else {
      if (!dryRun) {
        try {
          rewrite(path, decision.text)
        } catch (error) {
          result.failed.push({ name, why: `it could not be written (${error.message})` })
          continue
        }
      }
      result.retrofitted.push(name)
    }
  }
  return result
}

const USAGE =
  "usage: node scripts/retrofit-conversation-agents.mjs [--dry-run] [directory]"

function main() {
  const args = process.argv.slice(2)
  const dryRun = args.includes("--dry-run")
  const given = args.filter((argument) => argument !== "--dry-run")
  if (given.includes("--help") || given.includes("-h")) {
    console.log(USAGE)
    return
  }
  // A flag this script does not know is a mistake, not a directory. Taken as
  // one, `--help` reported "no conversation records at --help" and exited 0,
  // which is a script answering a question it was never asked.
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
  const directory = given[0] ?? join(namespaceRoot(), "conversations", "metadata")
  let result
  try {
    result = retrofit(directory, { dryRun })
  } catch (error) {
    if (error.code === "ENOENT") {
      console.log(`→ no conversation records at ${directory}; nothing to retrofit`)
      return
    }
    console.error(`→ could not retrofit ${directory}: ${error.message}`)
    if (error.code === "EACCES" || error.code === "EPERM")
      console.error("  these records are private to the user the gateway runs as")
    process.exit(1)
  }
  const verb = dryRun ? "would be given" : "given"
  console.log(`→ ${directory}`)
  console.log(`    ${result.retrofitted.length} ${verb} "agent": "${ORIGINAL_AGENT}"`)
  console.log(`    ${result.current} already name their agent`)
  for (const { name, why } of result.foreign) console.log(`    skipped ${name}: ${why}`)
  for (const { name, why } of result.failed) console.error(`    failed  ${name}: ${why}`)
  if (dryRun && result.retrofitted.length > 0)
    console.log("  rerun without --dry-run to write them")
  // Said after the tally, so the operator sees what did get done before the
  // failure is reported, and nonzero so a script running this one stops.
  if (result.failed.length > 0) {
    console.error("  fix those and run again; everything above them is already done")
    process.exit(1)
  }
}

const invoked = process.argv[1] ? realpathSync(process.argv[1]) : ""
if (invoked === realpathSync(fileURLToPath(import.meta.url))) main()
