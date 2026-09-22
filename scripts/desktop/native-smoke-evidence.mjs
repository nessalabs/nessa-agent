import { mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"

export const nativeSmokeEvidenceLimits = Object.freeze({
  errorBytes: 16 * 1024,
  errorDepth: 6,
  errorItems: 16,
  logsBytes: 64 * 1024,
  metadataBytes: 32 * 1024,
})

function utf8Prefix(value, maximumBytes) {
  let result = ""
  let bytes = 0
  for (const scalar of String(value)) {
    const size = Buffer.byteLength(scalar)
    if (bytes + size > maximumBytes) return result
    result += scalar
    bytes += size
  }
  return result
}

function utf8Tail(value, maximumBytes) {
  const bytes = Buffer.from(String(value))
  if (bytes.length <= maximumBytes) return bytes.toString("utf8")
  let start = bytes.length - maximumBytes
  while (start < bytes.length && (bytes[start] & 0xc0) === 0x80) start += 1
  return bytes.subarray(start).toString("utf8")
}

/** Render nested native-smoke failures without losing causes or cleanup errors. */
export function renderNativeSmokeError(error) {
  const lines = []
  const seen = new WeakSet()
  let items = 0
  let omitted = false

  const visit = (value, label, depth) => {
    if (items >= nativeSmokeEvidenceLimits.errorItems) {
      if (!omitted) lines.push(`${label}: [additional errors omitted]`)
      omitted = true
      return false
    }
    items += 1
    if (depth > nativeSmokeEvidenceLimits.errorDepth) {
      lines.push(`${label}: [cause depth exceeded]`)
      return true
    }
    if (value && typeof value === "object") {
      if (seen.has(value)) {
        lines.push(`${label}: [cycle]`)
        return true
      }
      seen.add(value)
    }

    const description =
      value instanceof Error
        ? (value.stack ?? `${value.name}: ${value.message}`)
        : String(value)
    lines.push(
      `${label}: ${utf8Prefix(description, nativeSmokeEvidenceLimits.errorBytes)}`,
    )
    if (value instanceof AggregateError) {
      let index = 0
      for (const nested of value.errors) {
        if (!visit(nested, `${label}.errors[${index}]`, depth + 1)) break
        index += 1
      }
    }
    if (value instanceof Error && value.cause !== undefined)
      visit(value.cause, `${label}.cause`, depth + 1)
    return true
  }

  visit(error, "failure", 0)
  return utf8Prefix(lines.join("\n"), nativeSmokeEvidenceLimits.errorBytes)
}

function selectedMetadata(metadata, errorBytes) {
  const text = (value, bytes = 512) =>
    value === undefined ? undefined : utf8Prefix(value, bytes)
  const numbers = (record) =>
    record && typeof record === "object"
      ? Object.fromEntries(
          Object.entries(record)
            .slice(0, 8)
            .map(([key, value]) => [text(key, 64), Number(value) || null]),
        )
      : undefined
  return {
    lifecyclePhase: text(metadata.lifecyclePhase),
    error: text(metadata.error, errorBytes),
    instance: text(metadata.instance, 256),
    ports: numbers(metadata.ports),
    pids: numbers(metadata.pids),
    sessionCreated: Boolean(metadata.sessionCreated),
    executableArtifacts:
      metadata.executableArtifacts && typeof metadata.executableArtifacts === "object"
        ? Object.fromEntries(
            Object.entries(metadata.executableArtifacts)
              .slice(0, 4)
              .map(([key, value]) => [text(key, 64), text(value, 1024)]),
          )
        : undefined,
  }
}

function boundedMetadata(metadata) {
  let errorBytes = nativeSmokeEvidenceLimits.errorBytes
  while (errorBytes >= 256) {
    const selected = selectedMetadata(metadata, errorBytes)
    const json = `${JSON.stringify(selected, null, 2)}\n`
    if (Buffer.byteLength(json) <= nativeSmokeEvidenceLimits.metadataBytes) return json
    errorBytes = Math.floor(errorBytes / 2)
  }
  return `${JSON.stringify({
    lifecyclePhase: utf8Prefix(metadata.lifecyclePhase ?? "unknown", 128),
    error: "failure metadata exceeded its byte budget",
    truncated: true,
  })}\n`
}

/** Retain bounded, non-secret diagnostics after the native smoke cleanup finishes. */
export function retainNativeSmokeFailure(root, instance, { logs, metadata }) {
  const directory = join(root, instance)
  mkdirSync(directory, { recursive: true, mode: 0o700 })
  writeFileSync(
    join(directory, "harness.log"),
    utf8Tail(logs, nativeSmokeEvidenceLimits.logsBytes),
    { mode: 0o600 },
  )
  writeFileSync(join(directory, "metadata.json"), boundedMetadata(metadata), {
    mode: 0o600,
  })
  return directory
}
