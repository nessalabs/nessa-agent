import { describe, expect, it, vi } from "vitest"
import {
  joinSubagentSources,
  joinedSubagentId,
  RepeatedSubagentSourceKey,
  splitJoinedSubagentId,
  type SubagentLog,
  type SubagentRead,
  type SubagentSource,
} from "./ports"
import type { Subagent } from "../model/subagent"

function child(id: string): Subagent {
  return {
    id,
    name: id,
    seed: id,
    tags: [],
    headline: id,
    activity: "idle",
    lifecycle: "open",
    since: 0,
    conversation: { messages: [], activity: null },
  }
}

function ready(subagents: readonly Subagent[]): SubagentRead {
  return { kind: "ready", subagents, unreadable: [] }
}

function source(read: SubagentRead): SubagentSource & {
  set(next: SubagentRead): void
  listeners: Set<() => void>
} {
  let current = read
  const listeners = new Set<() => void>()
  return {
    listeners,
    forSession: () => current,
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    set(next) {
      current = next
      for (const listener of listeners) listener()
    },
  }
}

function logger(): SubagentLog & { readonly warnings: string[] } {
  const warnings: string[] = []
  return { warnings, warn: (message) => warnings.push(message) }
}

describe("joined ids", () => {
  it("encodes a colon and a lone surrogate so two pairs cannot collide", () => {
    const colon = joinedSubagentId("a:b", "c")
    const other = joinedSubagentId("a", "b:c")
    expect(colon).not.toBe(other)
    expect(splitJoinedSubagentId(colon)).toEqual({ sourceKey: "a:b", sourceId: "c" })
    expect(splitJoinedSubagentId(other)).toEqual({ sourceKey: "a", sourceId: "b:c" })
    const surrogate = joinedSubagentId("k", "\uD800")
    expect(splitJoinedSubagentId(surrogate)).toEqual({
      sourceKey: "k",
      sourceId: "\uD800",
    })
    expect(surrogate.split(":").length).toBe(2)
  })
})

describe("joinSubagentSources", () => {
  it("refuses a repeated key before reading a source", () => {
    const first = source(ready([child("a")]))
    const read = vi.spyOn(first, "forSession")
    const listen = vi.spyOn(first, "subscribe")
    expect(() =>
      joinSubagentSources(
        [
          { key: "sample", source: first },
          { key: "sample", source: first },
        ],
        logger(),
      ),
    ).toThrow(RepeatedSubagentSourceKey)
    expect(read).not.toHaveBeenCalled()
    expect(listen).not.toHaveBeenCalled()
  })

  it("joins two sources under encoded ids", () => {
    const joined = joinSubagentSources(
      [
        { key: "sample", source: source(ready([child("one")])) },
        { key: "other", source: source(ready([child("two")])) },
      ],
      logger(),
    )
    const read = joined.forSession("parent")
    expect(read.kind).toBe("ready")
    if (read.kind !== "ready") return
    expect(read.subagents.map((each) => each.id)).toEqual([
      joinedSubagentId("sample", "one"),
      joinedSubagentId("other", "two"),
    ])
    expect(read.unreadable).toEqual([])
  })

  it("drops a repeated id when the update is taken in, and logs once per update", () => {
    const log = logger()
    const held = source(ready([child("x"), child("x"), child("y")]))
    const joined = joinSubagentSources([{ key: "sample", source: held }], log)
    const first = joined.forSession("parent")
    const again = joined.forSession("parent")
    expect(first).toBe(again)
    expect(first.kind).toBe("ready")
    if (first.kind !== "ready") return
    expect(
      first.subagents.map((each) => splitJoinedSubagentId(each.id)?.sourceId),
    ).toEqual(["x", "y"])
    expect(log.warnings).toEqual([
      'subagents: source "sample" repeated an id for "parent"',
    ])
    joined.forSession("parent")
    expect(log.warnings).toHaveLength(1)
    held.set(ready([child("x"), child("x")]))
    joined.forSession("parent")
    expect(log.warnings).toHaveLength(2)
  })

  it("follows one source updating", () => {
    const held = source(ready([child("one")]))
    const joined = joinSubagentSources([{ key: "sample", source: held }], logger())
    const heard: SubagentRead[] = []
    joined.subscribe(() => heard.push(joined.forSession("parent")))
    expect(ids(joined.forSession("parent"))).toEqual(["one"])
    held.set(ready([child("one"), child("two")]))
    expect(heard).toHaveLength(1)
    expect(ids(joined.forSession("parent"))).toEqual(["one", "two"])
  })

  it("stays unread while a source that has not failed is unread", () => {
    const joined = joinSubagentSources(
      [
        { key: "sample", source: source(ready([child("one")])) },
        { key: "other", source: source({ kind: "unread" }) },
      ],
      logger(),
    )
    expect(joined.forSession("parent")).toEqual({ kind: "unread" })
  })

  it("is ready with the failed key named when another source is ready", () => {
    const joined = joinSubagentSources(
      [
        { key: "sample", source: source(ready([child("one")])) },
        {
          key: "other",
          source: source({ kind: "failed", failure: { kind: "unavailable" } }),
        },
      ],
      logger(),
    )
    const read = joined.forSession("parent")
    expect(read.kind).toBe("ready")
    if (read.kind !== "ready") return
    expect(ids(read)).toEqual(["one"])
    expect(read.unreadable).toEqual(["other"])
  })

  it("fails when every source failed, rather than showing an empty list", () => {
    const joined = joinSubagentSources(
      [
        {
          key: "sample",
          source: source({ kind: "failed", failure: { kind: "unavailable" } }),
        },
        {
          key: "other",
          source: source({ kind: "failed", failure: { kind: "unavailable" } }),
        },
      ],
      logger(),
    )
    expect(joined.forSession("parent")).toEqual({
      kind: "failed",
      failure: { kind: "unavailable" },
    })
  })
})

function ids(read: SubagentRead): string[] {
  return read.kind === "ready"
    ? read.subagents.map((each) => splitJoinedSubagentId(each.id)?.sourceId ?? "")
    : []
}
