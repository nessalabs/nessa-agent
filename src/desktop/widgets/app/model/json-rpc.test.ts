/** Reading a frame's message into a bounded copy (#349, L27–L28). */
import { describe, expect, it } from "vitest"
import { copyJson, readEnvelope } from "./json-rpc"

describe("a frame's data, copied", () => {
  it("is rebuilt field by field, so changing the original changes nothing read", () => {
    const original = { a: [1, { b: "c" }], d: null, e: true }
    const copy = copyJson(original)
    expect(copy).toEqual(original)
    original.a.push(2)
    ;(original.a[1] as { b: string }).b = "changed"
    expect(copy).toEqual({ a: [1, { b: "c" }], d: null, e: true })
  })

  it("keeps __proto__ as a key of its own, never as the copy's prototype", () => {
    const hostile = JSON.parse('{"__proto__": {"polluted": true}, "x": 1}')
    const copy = copyJson(hostile) as Record<string, unknown>
    expect(Object.getPrototypeOf(copy)).toBe(Object.prototype)
    expect(Object.hasOwn(copy, "__proto__")).toBe(true)
    expect((copy as { polluted?: boolean }).polluted).toBeUndefined()
  })

  it("is refused for what is not JSON, and past its bounds", () => {
    const cycle: Record<string, unknown> = {}
    cycle.self = cycle
    for (const value of [
      undefined,
      () => {},
      Number.NaN,
      Infinity,
      new Uint8Array(2),
      new Date(0),
      new Map(),
      cycle,
      Symbol("s"),
      10n,
    ])
      expect(copyJson(value), String(value)).toBeUndefined()
    const bounds = { depth: 3, nodes: 5, stringLength: 4 }
    expect(copyJson([[[1]]], bounds)).toEqual([[[1]]])
    expect(copyJson([[[[1]]]], bounds)).toBeUndefined()
    expect(copyJson([1, 2, 3, 4, 5], bounds)).toBeUndefined()
    expect(copyJson("abcd", bounds)).toBe("abcd")
    expect(copyJson("abcde", bounds)).toBeUndefined()
    expect(copyJson({ abcde: 1 }, bounds)).toBeUndefined()
  })
})

describe("an envelope", () => {
  it("reads the spec's request, notification and responses", () => {
    expect(
      readEnvelope({
        jsonrpc: "2.0",
        id: 1,
        method: "ui/open-link",
        params: { url: "https://example.com" },
      }),
    ).toEqual({
      kind: "request",
      id: 1,
      method: "ui/open-link",
      params: { url: "https://example.com" },
    })
    expect(
      readEnvelope({
        jsonrpc: "2.0",
        method: "ui/notifications/size-changed",
        params: { width: 400, height: 600 },
      }),
    ).toEqual({
      kind: "notification",
      method: "ui/notifications/size-changed",
      params: { width: 400, height: 600 },
    })
    expect(readEnvelope({ jsonrpc: "2.0", id: 1, result: {} })).toEqual({
      kind: "result",
      id: 1,
      result: {},
    })
    expect(
      readEnvelope({
        jsonrpc: "2.0",
        id: "t",
        error: { code: -32000, message: "Teardown error" },
      }),
    ).toEqual({ kind: "error", id: "t", code: -32000 })
  })

  it("is malformed, with its id when one can be read, for anything else", () => {
    for (const [data, id] of [
      [null, null],
      ["{}", null],
      [{ id: 1, method: "ping" }, null],
      [{ jsonrpc: "1.0", id: 1, method: "ping" }, null],
      [{ jsonrpc: "2.0", id: null, method: "ping" }, null],
      [{ jsonrpc: "2.0", id: {}, method: "ping" }, null],
      [{ jsonrpc: "2.0", id: 1, method: 7 }, 1],
      [{ jsonrpc: "2.0", id: 1, method: "" }, 1],
      [{ jsonrpc: "2.0", id: 1, method: "x".repeat(257) }, 1],
      [{ jsonrpc: "2.0", id: 1, method: "ping", params: [] }, 1],
      [{ jsonrpc: "2.0", id: 1, method: "ping", params: "p" }, 1],
      [{ jsonrpc: "2.0", id: 2, result: {}, error: { code: 1 } }, 2],
      [{ jsonrpc: "2.0", id: 2, error: { code: "x" } }, 2],
      [{ jsonrpc: "2.0", method: "ping", extra: () => {} }, null],
    ] as const)
      expect(readEnvelope(data), JSON.stringify(data)).toEqual({ kind: "malformed", id })
  })
})
