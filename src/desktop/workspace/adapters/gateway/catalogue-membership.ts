/**
 * One catalogue observation of the conversations the caller owns (#596).
 *
 * Membership is the catalogue: a non-deleted descriptor whose payload is the
 * codec's object and whose summary is present and not archived. `running`
 * comes only from a list row that names the same id. Nothing here is published;
 * the source applies the whole result once, or applies nothing.
 *
 * A repeated raw key, a scope that is not this binding, a resolve that does
 * not echo the asked pass and descriptor, and a cursor that does not move
 * forward all reject the observation before anything is applied.
 */
import {
  maxCatalogueEntries,
  maxCataloguePayloadBytes,
  validPositiveReadEpoch,
  type CatalogueDescriptor,
  type CatalogueEntryKey,
  type CataloguePass,
  type CatalogueReadApi,
  type ConversationCatalogueHeadResult,
  type ConversationCatalogueResolveResult,
  type ConversationListResult,
  type ConversationSummary,
  type RecordScope,
} from "@nessa/client"
import publishedKeys from "../../../../../crates/nessa-protocol/src/conversation/catalogue-payload-keys.json"
import { WorkspaceSourceError } from "../../application/ports"

/** The receiver a catalogue read is admitted as. The panel does not have one. */
export interface CatalogueBinding {
  readonly receiverId: string
  readonly accessEpoch: string
}

export type CatalogueObservation =
  | { readonly kind: "list"; readonly result: ConversationListResult }
  | { readonly kind: "catalogue"; readonly summaries: readonly ConversationSummary[] }

/**
 * Object keys `encode` writes. The codec's test refuses a published list that
 * is not those keys, so this module reads the file instead of retyping it.
 */
const metadataKeys = keyList(publishedKeys.metadata)
const summaryKeys = keyList(publishedKeys.summary)
const scopeKeys = [
  "receiver",
  "origin",
  "stream",
  "incarnation",
  "schema",
  "accessEpoch",
] as const

const unavailable = () => new WorkspaceSourceError("unavailable")

function keyList(value: readonly string[]): readonly string[] {
  if (value.length === 0 || value.some((key) => typeof key !== "string"))
    throw new Error("catalogue payload keys")
  return value
}

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function own(value: object, key: string): boolean {
  return Object.hasOwn(value, key)
}

function exactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const present = Object.keys(value)
  return present.length === keys.length && keys.every((key) => Object.hasOwn(value, key))
}

function textOrNull(value: unknown): value is string | null {
  return value === null || typeof value === "string"
}

function whole(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
}

/** A cursor the product admits: not creation `"0"`, and not past the captured head. */
function cursorUsable(creation: string, head: string): boolean {
  if (creation === "0") return false
  try {
    return BigInt(creation) <= BigInt(head)
  } catch {
    return false
  }
}

/** Creation order, then id. Equal keys do not advance. */
function strictlyAfter(next: CatalogueEntryKey, previous: CatalogueEntryKey): boolean {
  const left = BigInt(next.creation)
  const right = BigInt(previous.creation)
  if (left > right) return true
  if (left < right) return false
  return next.id > previous.id
}

function sameScope(left: RecordScope, right: RecordScope): boolean {
  return scopeKeys.every(
    (key) => own(left, key) && own(right, key) && left[key] === right[key],
  )
}

/**
 * The head's scope is this binding. `accessEpoch` on the scope is the sync id
 * `epoch-` plus the numeric epoch (`passive_read_selector`). Stream and
 * organization are not on the binding, so they are not checked here.
 */
function scopeAdmitted(scope: RecordScope, binding: CatalogueBinding): boolean {
  if (!validPositiveReadEpoch(binding.accessEpoch)) return false
  return (
    own(scope, "receiver") &&
    own(scope, "accessEpoch") &&
    scope.receiver === binding.receiverId &&
    scope.accessEpoch === `epoch-${binding.accessEpoch}`
  )
}

type Scan = number | "duplicate" | "bad"

function skip(text: string, index: number): number {
  let i = index
  while (i < text.length) {
    const mark = text[i]
    if (mark !== " " && mark !== "\n" && mark !== "\r" && mark !== "\t") break
    i += 1
  }
  return i
}

function scanString(text: string, index: number): { end: number; value: string } | null {
  if (text[index] !== '"') return null
  let i = index + 1
  while (i < text.length) {
    const mark = text[i]
    if (mark === '"') {
      try {
        const value: unknown = JSON.parse(text.slice(index, i + 1))
        if (typeof value !== "string") return null
        return { end: i + 1, value }
      } catch {
        return null
      }
    }
    if (mark === "\\") {
      if (text[i + 1] === "u") {
        if (i + 5 >= text.length) return null
        i += 6
      } else {
        if (i + 1 >= text.length) return null
        i += 2
      }
      continue
    }
    if (mark < " ") return null
    i += 1
  }
  return null
}

function scanNumber(text: string, index: number): Scan {
  let i = index
  if (text[i] === "-") i += 1
  if (text[i] === "0") i += 1
  else if (text[i] >= "1" && text[i] <= "9") {
    while (text[i] >= "0" && text[i] <= "9") i += 1
  } else return "bad"
  if (text[i] === ".") {
    i += 1
    if (text[i] < "0" || text[i] > "9") return "bad"
    while (text[i] >= "0" && text[i] <= "9") i += 1
  }
  if (text[i] === "e" || text[i] === "E") {
    i += 1
    if (text[i] === "+" || text[i] === "-") i += 1
    if (text[i] < "0" || text[i] > "9") return "bad"
    while (text[i] >= "0" && text[i] <= "9") i += 1
  }
  return i
}

function scanValue(text: string, index: number): Scan {
  const start = skip(text, index)
  const mark = text[start]
  if (mark === "{") return scanObject(text, start)
  if (mark === "[") return scanArray(text, start)
  if (mark === '"') {
    const read = scanString(text, start)
    return read === null ? "bad" : read.end
  }
  if (mark === "t") return literal(text, start, "true")
  if (mark === "f") return literal(text, start, "false")
  if (mark === "n") return literal(text, start, "null")
  if (mark === "-" || (mark !== undefined && mark >= "0" && mark <= "9"))
    return scanNumber(text, start)
  return "bad"
}

function literal(text: string, index: number, word: string): Scan {
  return text.startsWith(word, index) ? index + word.length : "bad"
}

function scanArray(text: string, index: number): Scan {
  let i = skip(text, index + 1)
  if (text[i] === "]") return i + 1
  for (;;) {
    const value = scanValue(text, i)
    if (typeof value !== "number") return value
    i = skip(text, value)
    if (text[i] === "]") return i + 1
    if (text[i] !== ",") return "bad"
    i += 1
  }
}

function scanObject(text: string, index: number): Scan {
  let i = skip(text, index + 1)
  if (text[i] === "}") return i + 1
  const keys = new Set<string>()
  for (;;) {
    i = skip(text, i)
    const key = scanString(text, i)
    if (key === null) return "bad"
    if (keys.has(key.value)) return "duplicate"
    keys.add(key.value)
    i = skip(text, key.end)
    if (text[i] !== ":") return "bad"
    const value = scanValue(text, i + 1)
    if (typeof value !== "number") return value
    i = skip(text, value)
    if (text[i] === "}") return i + 1
    if (text[i] !== ",") return "bad"
    i += 1
  }
}

/** True when any object, at any depth, repeats a key once the escapes are decoded. */
function repeatedKey(text: string): boolean {
  const scanned = scanValue(text, 0)
  if (scanned === "duplicate") return true
  if (scanned === "bad") return true
  return skip(text, scanned) !== text.length
}

/**
 * The payload as one catalogue object for `expected` id. Absent means the
 * entry is not a session. Malformed rejects the whole observation.
 */
export function membershipOf(
  payload: Uint8Array,
  expected: string,
): ConversationSummary | "absent" | "malformed" {
  let text: string
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(payload)
  } catch {
    return "malformed"
  }
  if (repeatedKey(text)) return "malformed"
  let parsed: unknown
  try {
    parsed = JSON.parse(text)
  } catch {
    return "malformed"
  }
  if (!object(parsed) || !exactKeys(parsed, metadataKeys)) return "malformed"
  if (parsed.id !== expected || typeof parsed.id !== "string") return "malformed"
  if (!whole(parsed.createdAtMs)) return "malformed"
  if (!textOrNull(parsed.agent)) return "malformed"
  if (typeof parsed.model !== "string" || typeof parsed.approvalMode !== "string")
    return "malformed"
  if (parsed.summary === null) return "absent"
  if (!object(parsed.summary) || !exactKeys(parsed.summary, summaryKeys))
    return "malformed"
  const summary = parsed.summary
  if (!textOrNull(summary.title) || !textOrNull(summary.preview)) return "malformed"
  if (!whole(summary.updatedAtMs) || typeof summary.archived !== "boolean")
    return "malformed"
  if (summary.archived) return "absent"
  return {
    conversationId: parsed.id,
    title: summary.title,
    preview: summary.preview,
    createdAtMs: parsed.createdAtMs,
    updatedAtMs: summary.updatedAtMs,
    running: false,
    archived: false,
  }
}

function passEcho(request: CataloguePass, asked: CataloguePass): boolean {
  if (
    !own(request, "generation") ||
    !own(request, "completed") ||
    !own(request, "boundary") ||
    !own(request, "scope") ||
    !own(asked, "generation") ||
    !own(asked, "completed") ||
    !own(asked, "boundary") ||
    !own(asked, "scope")
  )
    return false
  if (
    request.generation !== asked.generation ||
    request.completed !== asked.completed ||
    request.boundary !== asked.boundary ||
    !sameScope(request.scope, asked.scope)
  )
    return false
  const askedCursor = own(asked, "cursor")
  const requestCursor = own(request, "cursor")
  if (!askedCursor) return !requestCursor && request.cursor === undefined
  if (!requestCursor || asked.cursor === undefined || request.cursor === undefined)
    return false
  return (
    own(request.cursor, "creation") &&
    own(request.cursor, "id") &&
    own(asked.cursor, "creation") &&
    own(asked.cursor, "id") &&
    request.cursor.creation === asked.cursor.creation &&
    request.cursor.id === asked.cursor.id
  )
}

function pageEcho(
  request: CataloguePass,
  asked: CataloguePass,
  maxEntries: number,
  echoedMax: number,
  scope: RecordScope,
  head: string,
): boolean {
  if (!passEcho(request, asked)) return false
  return (
    echoedMax === maxEntries &&
    request.generation === "1" &&
    request.completed === "0" &&
    request.boundary === head &&
    sameScope(request.scope, scope)
  )
}

function descriptorEcho(
  actual: CatalogueDescriptor,
  asked: CatalogueDescriptor,
): boolean {
  return (
    own(actual, "key") &&
    own(actual, "revision") &&
    own(actual, "deleted") &&
    own(actual.key, "creation") &&
    own(actual.key, "id") &&
    actual.key.creation === asked.key.creation &&
    actual.key.id === asked.key.id &&
    actual.revision === asked.revision &&
    actual.deleted === asked.deleted
  )
}

function entryKeyEcho(entry: CatalogueDescriptor, asked: CatalogueEntryKey): boolean {
  return (
    own(entry, "key") &&
    own(entry, "deleted") &&
    own(entry.key, "creation") &&
    own(entry.key, "id") &&
    entry.key.creation === asked.creation &&
    entry.key.id === asked.id
  )
}

async function walk(
  catalogue: CatalogueReadApi,
  binding: CatalogueBinding,
  discovered: ConversationCatalogueHeadResult,
): Promise<readonly ConversationSummary[]> {
  let pass: CataloguePass = {
    scope: discovered.scope,
    completed: "0",
    boundary: discovered.head,
    generation: "1",
  }
  const summaries: ConversationSummary[] = []
  const seen = new Set<string>()
  for (;;) {
    const page = await catalogue.manifest({
      request: { pass, maxEntries: maxCatalogueEntries },
      accessEpoch: binding.accessEpoch,
    })
    if (
      !pageEcho(
        page.request.pass,
        pass,
        maxCatalogueEntries,
        page.request.maxEntries,
        discovered.scope,
        discovered.head,
      )
    )
      throw unavailable()
    if (page.entries.length === 0 && page.hasMore) throw unavailable()
    if (page.entries.length > maxCatalogueEntries) throw unavailable()
    for (const descriptor of page.entries) {
      const summary = await one(catalogue, binding, pass, descriptor)
      if (summary === "malformed") throw unavailable()
      if (summary === "absent") continue
      if (seen.has(summary.conversationId)) throw unavailable()
      seen.add(summary.conversationId)
      summaries.push(summary)
    }
    if (!page.hasMore) return summaries
    const last = page.entries[page.entries.length - 1]
    if (!last || !cursorUsable(last.key.creation, discovered.head)) throw unavailable()
    const next = { creation: last.key.creation, id: last.key.id }
    const previous = pass.cursor
    if (previous !== undefined && !strictlyAfter(next, previous)) throw unavailable()
    pass = { ...pass, cursor: next }
  }
}

async function one(
  catalogue: CatalogueReadApi,
  binding: CatalogueBinding,
  pass: CataloguePass,
  descriptor: CatalogueDescriptor,
): Promise<ConversationSummary | "absent" | "malformed"> {
  if (descriptor.deleted) return "absent"
  const resolved = await catalogue.resolve({
    pass,
    descriptor,
    maxPayloadBytes: maxCataloguePayloadBytes,
    accessEpoch: binding.accessEpoch,
  })
  if (!echoes(resolved, pass, descriptor)) return "malformed"
  if (resolved.entry.deleted) return "absent"
  return membershipOf(resolved.payload, descriptor.key.id)
}

function echoes(
  resolved: ConversationCatalogueResolveResult,
  pass: CataloguePass,
  descriptor: CatalogueDescriptor,
): boolean {
  return (
    passEcho(resolved.pass, pass) &&
    descriptorEcho(resolved.descriptor, descriptor) &&
    entryKeyEcho(resolved.entry, descriptor.key)
  )
}

function withRunning(
  summaries: readonly ConversationSummary[],
  listed: ConversationListResult,
): readonly ConversationSummary[] {
  const running = new Map<string, boolean>()
  for (const row of listed.conversations) running.set(row.conversationId, row.running)
  return summaries.map((summary) => {
    if (!running.has(summary.conversationId)) return summary
    return { ...summary, running: running.get(summary.conversationId) === true }
  })
}

/**
 * The owned summaries, or the list alone when the catalogue head is `"0"`.
 * The list runs only after a walk that succeeded, and only when the head's
 * scope admits this binding. Either failure throws and leaves the caller
 * nothing to publish.
 */
export async function catalogueObservation(
  catalogue: CatalogueReadApi,
  binding: CatalogueBinding,
  list: () => Promise<ConversationListResult>,
): Promise<CatalogueObservation> {
  try {
    const discovered = await catalogue.head({
      receiverId: binding.receiverId,
      accessEpoch: binding.accessEpoch,
    })
    if (!scopeAdmitted(discovered.scope, binding)) throw unavailable()
    if (discovered.head === "0") return { kind: "list", result: await list() }
    const summaries = await walk(catalogue, binding, discovered)
    return { kind: "catalogue", summaries: withRunning(summaries, await list()) }
  } catch (error) {
    if (error instanceof WorkspaceSourceError) throw error
    throw unavailable()
  }
}
