/**
 * One catalogue observation of the conversations the caller owns (#596).
 *
 * Membership is the catalogue: a non-deleted descriptor whose payload is the
 * codec's object and whose summary is present and not archived. `running`
 * comes only from a list row that names the same id. Nothing here is published;
 * the source applies the whole result once, or applies nothing.
 */
import {
  maxCatalogueEntries,
  maxCataloguePayloadBytes,
  type CatalogueDescriptor,
  type CataloguePass,
  type CatalogueReadApi,
  type ConversationCatalogueHeadResult,
  type ConversationListResult,
  type ConversationSummary,
  type RecordScope,
} from "@nessa/client"
import { WorkspaceSourceError } from "../../application/ports"

/** The receiver a catalogue read is admitted as. The panel does not have one. */
export interface CatalogueBinding {
  readonly receiverId: string
  readonly accessEpoch: string
}

export type CatalogueObservation =
  | { readonly kind: "list"; readonly result: ConversationListResult }
  | { readonly kind: "catalogue"; readonly summaries: readonly ConversationSummary[] }

const metadataKeys = [
  "id",
  "createdAtMs",
  "agent",
  "model",
  "approvalMode",
  "summary",
] as const
const summaryKeys = ["title", "preview", "updatedAtMs", "archived"] as const
const scopeKeys = [
  "receiver",
  "origin",
  "stream",
  "incarnation",
  "schema",
  "accessEpoch",
] as const

const unavailable = () => new WorkspaceSourceError("unavailable")

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
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

function sameScope(left: RecordScope, right: RecordScope): boolean {
  return scopeKeys.every(
    (key) =>
      Object.hasOwn(left, key) && Object.hasOwn(right, key) && left[key] === right[key],
  )
}

/**
 * The payload as one catalogue object for `expected` id. Absent means the
 * entry is not a session. Malformed rejects the whole observation.
 */
export function membershipOf(
  payload: Uint8Array,
  expected: string,
): ConversationSummary | "absent" | "malformed" {
  let parsed: unknown
  try {
    parsed = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(payload))
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

function pageEcho(
  request: CataloguePass,
  asked: CataloguePass,
  maxEntries: number,
  echoedMax: number,
  scope: RecordScope,
  head: string,
): boolean {
  return (
    echoedMax === maxEntries &&
    request.generation === "1" &&
    request.completed === "0" &&
    request.boundary === head &&
    request.generation === asked.generation &&
    request.completed === asked.completed &&
    request.boundary === asked.boundary &&
    sameScope(request.scope, scope) &&
    (asked.cursor === undefined
      ? request.cursor === undefined
      : request.cursor?.creation === asked.cursor.creation &&
        request.cursor.id === asked.cursor.id)
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
    pass = { ...pass, cursor: { creation: last.key.creation, id: last.key.id } }
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
  if (resolved.entry.key.id !== descriptor.key.id) return "malformed"
  if (resolved.entry.deleted) return "absent"
  return membershipOf(resolved.payload, descriptor.key.id)
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
 * The list runs only after a walk that succeeded. Either failure throws and
 * leaves the caller nothing to publish.
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
    if (discovered.head === "0") return { kind: "list", result: await list() }
    const summaries = await walk(catalogue, binding, discovered)
    return { kind: "catalogue", summaries: withRunning(summaries, await list()) }
  } catch (error) {
    if (error instanceof WorkspaceSourceError) throw error
    throw unavailable()
  }
}
