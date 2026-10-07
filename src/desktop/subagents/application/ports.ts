/**
 * Where a conversation's subagents are read from, and the join of several
 * such sources. Read only: a source has no send. `forSession` returns one
 * stable value until `subscribe` fires (`join.test.ts`).
 *
 * ```text
 *   source.forSession ──▶ unread | ready(subagents, unreadable) | failed
 *   joinSubagentSources([{ key, source }, …])
 *        │  repeated key: throws before any source is read
 *        ▼
 *   one SubagentSource: unread while any source that has not failed is unread;
 *   otherwise ready, with the failed sources' keys in unreadable
 * ```
 */
import { decodeId, encodeId } from "../../model/id-encoding"
import type { Subagent } from "../model/subagent"

/** Why a source could not be read. One kind: it is not available. */
export interface SubagentFailure {
  readonly kind: "unavailable"
}

/**
 * What a source says of one conversation. `unreadable` names sources that
 * failed; a single source leaves it empty. `ready`'s subagents carry that
 * source's own ids until a join rewrites them.
 */
export type SubagentRead =
  | { readonly kind: "unread" }
  | {
      readonly kind: "ready"
      readonly subagents: readonly Subagent[]
      readonly unreadable: readonly string[]
    }
  | { readonly kind: "failed"; readonly failure: SubagentFailure }

/**
 * A source of subagents. `forSession` answers from what it already holds and
 * returns the same value until it notifies. `subscribe` hears that it changed.
 */
export interface SubagentSource {
  forSession(sessionId: string): SubagentRead
  subscribe(listener: () => void): () => void
}

/** Where a join says that a source repeated an id. */
export interface SubagentLog {
  warn(message: string): void
}

/** The plugin id the panel is registered under. The sample's widget card uses the same word. */
export const subagentsPluginId = "subagents"

/** The key the sample source is joined under. */
export const sampleSubagentKey = "sample"

/**
 * A joined id: the source's key, `:`, and the source's own id, each through
 * the desktop's encoder so a `:` or a lone surrogate in either cannot collide
 * with another pair (`join.test.ts`).
 */
export function joinedSubagentId(sourceKey: string, sourceId: string): string {
  return `${encodeId(sourceKey)}:${encodeId(sourceId)}`
}

/** The key and source id a joined id was built from, or `null` when it was not. */
export function splitJoinedSubagentId(
  joined: string,
): { readonly sourceKey: string; readonly sourceId: string } | null {
  const colon = joined.indexOf(":")
  if (colon < 0) return null
  const sourceKey = decodeId(joined.slice(0, colon))
  const sourceId = decodeId(joined.slice(colon + 1))
  if (sourceKey === null || sourceId === null) return null
  return { sourceKey, sourceId }
}

/** Raised when `joinSubagentSources` is given one key twice, before it reads a source. */
export class RepeatedSubagentSourceKey extends Error {
  readonly key: string

  constructor(key: string) {
    super(`A subagent source key is repeated: ${key}`)
    this.name = "RepeatedSubagentSourceKey"
    this.key = key
  }
}

const unavailable: SubagentFailure = { kind: "unavailable" }

interface Part {
  readonly key: string
  readonly source: SubagentSource
}

/**
 * One source over several, keyed. A repeated key throws before any lookup
 * is built and before any source is read. While any source that has not
 * failed is unread, the join is unread. Once the rest are ready, it is ready
 * with their subagents and the keys of those that failed. Every source
 * failing is `failed`, not an empty list.
 *
 * A source that repeats an id is taken in once per update: the first copy
 * is kept, and `log` is told once for that update. The joined read is cached
 * until a source notifies, so a later read of the same update does not tell
 * it again.
 */
export function joinSubagentSources(
  parts: readonly Part[],
  log: SubagentLog,
): SubagentSource {
  const seen = new Set<string>()
  for (const part of parts) {
    if (seen.has(part.key)) throw new RepeatedSubagentSourceKey(part.key)
    seen.add(part.key)
  }

  const generation = new Map<string, number>()
  const combined = new Map<
    string,
    { readonly stamp: string; readonly read: SubagentRead }
  >()
  const listeners = new Set<() => void>()

  // Subscribed for the join's life. Composition holds one join for the
  // window; an update that lands while nobody is listening is still taken
  // in on the next read, because the generation moved and the cache dropped.
  for (const part of parts) {
    part.source.subscribe(() => {
      generation.set(part.key, (generation.get(part.key) ?? 0) + 1)
      combined.clear()
      for (const listener of listeners) listener()
    })
  }

  const stamp = () =>
    parts.map((part) => String(generation.get(part.key) ?? 0)).join("\0")

  return {
    forSession(sessionId) {
      const now = stamp()
      const hit = combined.get(sessionId)
      if (hit && hit.stamp === now) return hit.read
      const read = combine(
        parts.map((part) => ({
          key: part.key,
          read: admit(part.key, sessionId, part.source.forSession(sessionId), log),
        })),
      )
      combined.set(sessionId, { stamp: now, read })
      return read
    },
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
  }
}

/** A source that has not read anything. A window without the sample uses it, and does not pretend the list is empty. */
export function unreadSubagentSource(): SubagentSource {
  const unread: SubagentRead = { kind: "unread" }
  return {
    forSession() {
      return unread
    },
    subscribe() {
      return () => {}
    },
  }
}

function admit(
  key: string,
  sessionId: string,
  read: SubagentRead,
  log: SubagentLog,
): SubagentRead {
  if (read.kind !== "ready") return read
  const seen = new Set<string>()
  const subagents: Subagent[] = []
  let dropped = false
  for (const subagent of read.subagents) {
    if (seen.has(subagent.id)) {
      dropped = true
      continue
    }
    seen.add(subagent.id)
    subagents.push({ ...subagent, id: joinedSubagentId(key, subagent.id) })
  }
  if (dropped) log.warn(`subagents: source "${key}" repeated an id for "${sessionId}"`)
  return { kind: "ready", subagents, unreadable: read.unreadable }
}

function combine(
  reads: readonly { readonly key: string; readonly read: SubagentRead }[],
): SubagentRead {
  let waiting = false
  let ready = 0
  const subagents: Subagent[] = []
  const unreadable: string[] = []
  for (const part of reads) {
    if (part.read.kind === "unread") waiting = true
    else if (part.read.kind === "failed") pushKey(unreadable, part.key)
    else {
      ready += 1
      subagents.push(...part.read.subagents)
      for (const key of part.read.unreadable) pushKey(unreadable, key)
    }
  }
  if (waiting) return { kind: "unread" }
  if (ready === 0 && unreadable.length > 0)
    return { kind: "failed", failure: unavailable }
  return { kind: "ready", subagents, unreadable }
}

function pushKey(keys: string[], key: string) {
  if (!keys.includes(key)) keys.push(key)
}
