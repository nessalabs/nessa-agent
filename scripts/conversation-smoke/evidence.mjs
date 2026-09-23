import assert from "node:assert/strict"
import { readFileSync } from "node:fs"

const correlationPattern = /^fixtureCorrelation:([a-z0-9][a-z0-9-]{0,127})$/mu

/** Read the one deliberate correlation marker carried in fixture prompt text. */
export function fixtureCorrelation(blocks) {
  const text = blocks
    .filter((block) => block?.type === "text" && typeof block.text === "string")
    .map((block) => block.text)
    .join("\n")
  const matches = [...text.matchAll(new RegExp(correlationPattern.source, "gmu"))]
  assert.equal(matches.length, 1, "fixture prompt must carry one correlation marker")
  return matches[0][1]
}

export function readEvidence(path) {
  const text = readFileSync(path, "utf8")
  if (!text.trim()) return []
  return text
    .trimEnd()
    .split("\n")
    .map((line) => JSON.parse(line))
}

export function evidenceFor(events, type, providerSessionId, expectedExecutionId) {
  return events.filter(
    (event) =>
      event.type === type &&
      event.providerSessionId === providerSessionId &&
      event.expectedExecutionId === expectedExecutionId,
  )
}

export async function waitFor(read, accept, description, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs
  let value
  while (Date.now() < deadline) {
    value = await read()
    if (accept(value)) return value
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
  throw new Error(`timed out waiting for ${description}: ${JSON.stringify(value)}`)
}
