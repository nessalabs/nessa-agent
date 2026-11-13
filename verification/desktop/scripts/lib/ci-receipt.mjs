/** A bounded transport for the existing desktop producer's results, not a verdict owner. */
import { createHash } from "node:crypto"
import { gzipSync, gunzipSync } from "node:zlib"
import { budgetMs } from "./perf.mjs"

export const receiptPartSize = 3500
export const receiptPartsPerGroup = 8
export const receiptGroups = 3
export const receiptMaxRawBytes = 64 * 1024 * 1024
export const receiptMaxProjectionBytes = 32 * 1024 * 1024
const manifestTitle = "Desktop verification receipt manifest"
const partTitle = "Desktop verification receipt "
const annotationLimit = 4096
const omitted = [
  "successful functional details",
  "performance runs[].sample and baseRuns[].sample",
]

const hash = (value) => createHash("sha256").update(value).digest("hex")
const object = (value) =>
  value !== null && typeof value === "object" && !Array.isArray(value)
const own = (value, key) => object(value) && Object.hasOwn(value, key)

function sourceOf(source) {
  if (
    !own(source, "commit") ||
    typeof source.commit !== "string" ||
    !/^[a-f0-9]{40}$/.test(source.commit) ||
    !own(source, "run") ||
    typeof source.run !== "string" ||
    !/^\d+$/.test(source.run) ||
    !own(source, "attempt") ||
    typeof source.attempt !== "string" ||
    !/^\d+$/.test(source.attempt)
  )
    throw new Error("desktop receipt requires its GitHub commit, run and attempt")
  return { commit: source.commit, run: source.run, attempt: source.attempt }
}

/** Failed and skipped records retain their complete causal evidence. */
function functionalRecord(record) {
  if (
    !object(record) ||
    !own(record, "ok") ||
    record.ok !== true ||
    ["error", "skipped", "couldNotRun", "cannotRun"].some((key) => own(record, key)) ||
    (own(record, "failures") &&
      (!Array.isArray(record.failures) || record.failures.length !== 0))
  )
    return record
  return Object.fromEntries(
    ["name", "engine", "layout", "width", "height", "profile", "ok", "failures"]
      .filter((key) => own(record, key))
      .map((key) => [key, record[key]]),
  )
}

/** Slow-frame entries already own attribution; only the dense sample is omitted. */
function measuredRuns(runs) {
  if (!Array.isArray(runs)) return runs
  return runs.map((run) =>
    object(run)
      ? Object.fromEntries(Object.entries(run).filter(([key]) => key !== "sample"))
      : run,
  )
}

function checkReceipt(document, performance) {
  if (!object(document) || !own(document, "results") || !Array.isArray(document.results))
    return document
  return {
    ...document,
    results: document.results.map((record) => {
      if (!performance) return functionalRecord(record)
      if (!object(record)) return record
      const copied = { ...record }
      for (const key of ["runs", "baseRuns"])
        if (own(record, key)) copied[key] = measuredRuns(record[key])
      return copied
    }),
  }
}

export function incompleteReceipt(source, reason, extra = {}) {
  return {
    manifest: {
      ...extra,
      kind: "desktop-verification-receipt",
      source: sourceOf(source),
      status: "incomplete",
      reason,
    },
    parts: [],
  }
}

/** Copy published outcomes and measurements without recalculating any budget or result. */
export function desktopReceipt(raw, source) {
  source = sourceOf(source)
  if (!Buffer.isBuffer(raw) && typeof raw !== "string")
    return incompleteReceipt(source, "invalid-input")
  const rawBytes = Buffer.isBuffer(raw) ? raw.length : Buffer.byteLength(raw, "utf8")
  if (rawBytes > receiptMaxRawBytes)
    return incompleteReceipt(source, "raw-capacity-exceeded", {
      rawBytesAtLeast: rawBytes,
    })
  const input = Buffer.isBuffer(raw) ? raw : Buffer.from(raw, "utf8")
  const rawSha256 = hash(input)
  let document
  try {
    document = JSON.parse(
      new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(input),
    )
  } catch {
    return incompleteReceipt(source, "invalid-json", { rawSha256 })
  }
  if (
    !own(document, "check") ||
    document.check !== "run-all" ||
    !own(document, "ok") ||
    typeof document.ok !== "boolean" ||
    !own(document, "summary") ||
    !Array.isArray(document.summary) ||
    !own(document, "checks") ||
    !object(document.checks)
  )
    return incompleteReceipt(source, "invalid-run-all-document", { rawSha256 })
  const projection = {
    kind: "desktop-functional-outcomes-and-performance",
    source,
    budgetMs,
    check: document.check,
    ok: document.ok,
    summary: document.summary,
    checks: Object.fromEntries(
      Object.entries(document.checks).map(([name, check]) => [
        name,
        checkReceipt(check, name === "perf-budget"),
      ]),
    ),
  }
  const bytes = Buffer.from(JSON.stringify(projection), "utf8")
  if (bytes.length > receiptMaxProjectionBytes)
    return incompleteReceipt(source, "projection-capacity-exceeded", {
      rawSha256,
      projectionBytes: bytes.length,
    })
  const encoded = gzipSync(bytes).toString("base64")
  const count = Math.ceil(encoded.length / receiptPartSize)
  const manifest = {
    kind: "desktop-verification-receipt",
    source,
    status: "prepared",
    producerOk: document.ok,
    rawSha256,
    projectionSha256: hash(bytes),
    projectionBytes: bytes.length,
    encoding: "gzip+base64",
    parts: count,
    groups: receiptGroups,
    partsPerGroup: receiptPartsPerGroup,
    omitted: [...omitted],
  }
  if (count > receiptGroups * receiptPartsPerGroup)
    return incompleteReceipt(source, "capacity-exceeded", manifest)
  const parts = []
  for (let at = 0; at < encoded.length; at += receiptPartSize)
    parts.push(encoded.slice(at, at + receiptPartSize))
  return { manifest, parts }
}

/** ASCII bodies cannot inject workflow commands or be changed by percent decoding. */
function notice(title, body) {
  const line = "::notice title=" + title + "::" + body
  if (Buffer.byteLength(line, "utf8") > annotationLimit)
    throw new Error("desktop receipt annotation exceeds runner capacity")
  return line
}

export function receiptNotices(receipt, section) {
  const { manifest, parts } = receipt
  if (section === "manifest")
    return [
      notice(
        manifestTitle,
        Buffer.from(JSON.stringify(manifest), "utf8").toString("base64"),
      ),
    ]
  const selected = /^group([1-9]\d*)$/.exec(section)
  if (!selected || Number(selected[1]) > receiptGroups)
    throw new Error("expected manifest or group1 through group" + receiptGroups)
  if (manifest.status !== "prepared") return []
  const group = Number(selected[1]) - 1
  const start = group * receiptPartsPerGroup
  return parts
    .slice(start, start + receiptPartsPerGroup)
    .map((part, offset) =>
      notice(
        partTitle +
          manifest.projectionSha256 +
          " " +
          (start + offset + 1) +
          "/" +
          parts.length,
        part,
      ),
    )
}

function base64(value) {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length % 4 !== 0 ||
    !/^[A-Za-z0-9+/]*={0,2}$/.test(value)
  )
    throw new Error("invalid desktop receipt encoding")
  return Buffer.from(value, "base64")
}

/** Read a terminal job's fully paginated annotations; never infer missing delivery. */
export function decodeDesktopReceipt(annotations, expectedSource) {
  expectedSource = sourceOf(expectedSource)
  if (!Array.isArray(annotations)) throw new Error("expected all job annotations")
  const owned = []
  for (const row of annotations)
    if (
      own(row, "title") &&
      typeof row.title === "string" &&
      row.title.startsWith(partTitle)
    ) {
      if (owned.length === receiptGroups * receiptPartsPerGroup + 1)
        throw new Error("too many desktop receipt annotations")
      owned.push(row)
    }
  const manifests = owned.filter((row) => row.title === manifestTitle)
  if (
    manifests.length !== 1 ||
    !own(manifests[0], "message") ||
    typeof manifests[0].message !== "string" ||
    manifests[0].message.length > annotationLimit
  )
    throw new Error("expected exactly one desktop receipt manifest")
  const manifest = JSON.parse(
    new TextDecoder("utf-8", { fatal: true }).decode(base64(manifests[0].message)),
  )
  if (
    !own(manifest, "kind") ||
    manifest.kind !== "desktop-verification-receipt" ||
    !own(manifest, "source") ||
    JSON.stringify(sourceOf(manifest.source)) !== JSON.stringify(expectedSource)
  )
    throw new Error("desktop receipt source does not match the requested run")
  if (!own(manifest, "status") || manifest.status !== "prepared")
    throw new Error("desktop receipt is incomplete")
  if (
    !own(manifest, "encoding") ||
    manifest.encoding !== "gzip+base64" ||
    !own(manifest, "projectionSha256") ||
    !/^[a-f0-9]{64}$/.test(manifest.projectionSha256) ||
    !own(manifest, "rawSha256") ||
    !/^[a-f0-9]{64}$/.test(manifest.rawSha256) ||
    !own(manifest, "parts") ||
    !Number.isSafeInteger(manifest.parts) ||
    manifest.parts < 1 ||
    manifest.parts > receiptGroups * receiptPartsPerGroup ||
    !own(manifest, "projectionBytes") ||
    !Number.isSafeInteger(manifest.projectionBytes) ||
    manifest.projectionBytes < 1 ||
    manifest.projectionBytes > receiptMaxProjectionBytes ||
    !own(manifest, "producerOk") ||
    typeof manifest.producerOk !== "boolean" ||
    !own(manifest, "groups") ||
    manifest.groups !== receiptGroups ||
    !own(manifest, "partsPerGroup") ||
    manifest.partsPerGroup !== receiptPartsPerGroup ||
    !own(manifest, "omitted") ||
    JSON.stringify(manifest.omitted) !== JSON.stringify(omitted)
  )
    throw new Error("invalid desktop receipt manifest")
  const parts = new Map()
  for (const row of owned) {
    if (row.title === manifestTitle) continue
    const matched = /^([a-f0-9]{64}) ([1-9]\d*)\/([1-9]\d*)$/.exec(
      row.title.slice(partTitle.length),
    )
    if (
      !matched ||
      matched[1] !== manifest.projectionSha256 ||
      Number(matched[3]) !== manifest.parts ||
      Number(matched[2]) > manifest.parts ||
      parts.has(Number(matched[2])) ||
      !own(row, "message") ||
      typeof row.message !== "string" ||
      row.message.length > receiptPartSize
    )
      throw new Error("duplicate, mixed or invalid desktop receipt part")
    base64(row.message)
    parts.set(Number(matched[2]), row.message)
  }
  if (parts.size !== manifest.parts) throw new Error("desktop receipt parts are missing")
  const encoded = Array.from({ length: manifest.parts }, (_, index) =>
    parts.get(index + 1),
  ).join("")
  const bytes = gunzipSync(base64(encoded), { maxOutputLength: manifest.projectionBytes })
  if (
    bytes.length !== manifest.projectionBytes ||
    hash(bytes) !== manifest.projectionSha256
  )
    throw new Error("desktop receipt content does not match its manifest")
  const projection = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes))
  if (
    !own(projection, "kind") ||
    projection.kind !== "desktop-functional-outcomes-and-performance" ||
    !own(projection, "check") ||
    projection.check !== "run-all" ||
    !own(projection, "source") ||
    JSON.stringify(sourceOf(projection.source)) !== JSON.stringify(expectedSource) ||
    !own(projection, "ok") ||
    typeof projection.ok !== "boolean" ||
    projection.ok !== manifest.producerOk
  )
    throw new Error("desktop receipt facts do not match its manifest")
  return { manifest, projection }
}
