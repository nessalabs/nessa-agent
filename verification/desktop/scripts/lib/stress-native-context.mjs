/** Read-only evidence for the stress check's focused native-context boundary. */
import { readFileSync, readdirSync } from "node:fs"
import { join } from "node:path"

function read(path) {
  try {
    return readFileSync(path, "utf8")
  } catch (error) {
    if (error.code === "ENOENT") return null
    throw error
  }
}

/** Complete trace lines only: a writer may still be appending its final line. */
export function nativeContextEvents(directory) {
  const text = read(join(directory, "events.jsonl")) ?? ""
  const complete = text.slice(0, text.lastIndexOf("\n") + 1)
  return complete
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line))
}

/** Inspect fixture-owned used markers without duplicating its filename mapping. */
export function nativeContextExists(directory, sessionId) {
  return readdirSync(directory, { withFileTypes: true }).some(
    (entry) =>
      entry.isFile() &&
      entry.name.endsWith(".used") &&
      read(join(directory, entry.name)) === sessionId,
  )
}

/**
 * Audit identity spans process restarts; attachment generation alone does not.
 * Only the selected conversation's publication/admission facts participate.
 */
export function contextAudit(directory, conversationId) {
  const published = new Set()
  const admitted = new Set()
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (!entry.isFile() || !entry.name.endsWith(".json")) continue
    const bytes = read(join(directory, entry.name))
    if (bytes === null) continue
    const row = JSON.parse(bytes)
    const record = row.record
    if (record?.sessionId !== conversationId) continue
    const identity = JSON.stringify([row.writerId, row.recordId])
    if (typeof row.writerId !== "string" || typeof row.recordId !== "string")
      throw new Error("focused context audit lacks its durable record identity")
    if (record.kind === "attachment_transition" && record.after === "context_published")
      published.add(identity)
    if (record.kind === "queue_admitted") admitted.add(identity)
  }
  return { published, admitted }
}

/** Record identities present after a boundary that were absent before it. */
export function newContextEvidence(before, after) {
  return [...after].filter((identity) => !before.has(identity))
}
