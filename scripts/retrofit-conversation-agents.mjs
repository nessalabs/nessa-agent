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

/** Replace a record's bytes without ever leaving a half-written one in its place. */
function rewrite(path, text) {
  const temporary = `${path}.${randomUUID()}.tmp`
  try {
    writeFileSync(temporary, text, { mode: 0o600, flag: "wx" })
    renameSync(temporary, path)
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
 * @returns {{retrofitted: string[], current: number, foreign: Array<{name: string, why: string}>}}
 */
export function retrofit(directory, { dryRun = false } = {}) {
  const names = readdirSync(directory).filter((name) => name.endsWith(".json"))
  const result = { retrofitted: [], current: 0, foreign: [] }
  for (const name of names.sort()) {
    const path = join(directory, name)
    const decision = plan(readFileSync(path, "utf8"))
    if (decision.state === "current") result.current += 1
    else if (decision.state === "foreign")
      result.foreign.push({ name, why: decision.why })
    else {
      if (!dryRun) rewrite(path, decision.text)
      result.retrofitted.push(name)
    }
  }
  return result
}

function main() {
  const args = process.argv.slice(2)
  const dryRun = args.includes("--dry-run")
  const given = args.filter((argument) => argument !== "--dry-run")
  if (given.length > 1) {
    console.error(
      "usage: node scripts/retrofit-conversation-agents.mjs [--dry-run] [directory]",
    )
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
  if (dryRun && result.retrofitted.length > 0)
    console.log("  rerun without --dry-run to write them")
}

const invoked = process.argv[1] ? realpathSync(process.argv[1]) : ""
if (invoked === realpathSync(fileURLToPath(import.meta.url))) main()
