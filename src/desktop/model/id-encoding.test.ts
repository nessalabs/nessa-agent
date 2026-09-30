import { describe, expect, it } from "vitest"
import { decodeId, encodeId } from "./id-encoding"
import { generatedStrings } from "./testing"

const ids = generatedStrings(4000, 0x326)
/** What an encoding may contain: the unreserved characters and `%XXXX` escapes. */
const alphabet = /^(?:[A-Za-z0-9\-._~]|%[0-9A-F]{4})*$/

describe("encoding an id", () => {
  it("leaves the unreserved characters as they are and escapes every other code unit", () => {
    expect(encodeId("Session-1._~")).toBe("Session-1._~")
    expect(encodeId("a:b")).toBe("a%003Ab")
    expect(encodeId("%")).toBe("%0025")
    expect(encodeId("é")).toBe("%00E9")
    expect(encodeId("😀")).toBe("%D83D%DE00")
    expect(encodeId("")).toBe("")
  })

  it("encodes a lone surrogate, where encodeURIComponent throws", () => {
    expect(() => encodeURIComponent("\uD800")).toThrow()
    expect(encodeId("\uD800")).toBe("%D800")
    expect(encodeId("x\uDFFF")).toBe("x%DFFF")
    expect(decodeId("%D800")).toBe("\uD800")
  })

  it("round-trips every string, in its alphabet, never writing a separator", () => {
    for (const id of ids) {
      const encoded = encodeId(id)
      expect(encoded, JSON.stringify(id)).toMatch(alphabet)
      expect(decodeId(encoded), JSON.stringify(id)).toBe(id)
    }
  })

  it("is one-to-one: two strings never share an encoding", () => {
    const seen = new Map<string, string>()
    for (const id of ids) {
      const encoded = encodeId(id)
      const before = seen.get(encoded)
      if (before !== undefined) expect(before, JSON.stringify(encoded)).toBe(id)
      seen.set(encoded, id)
    }
    expect(new Set(ids).size).toBe(seen.size)
  })
})

describe("decoding an id", () => {
  it("reads back only what encodeId writes: every string has one encoding", () => {
    expect(decodeId("%0041")).toBeNull() // "A" is written as itself
    expect(decodeId("%00e9")).toBeNull() // lowercase digits
    expect(decodeId("%E9")).toBeNull() // two digits, not four
    expect(decodeId("%")).toBeNull()
    expect(decodeId("a:b")).toBeNull()
    expect(decodeId("a b")).toBeNull()
    expect(decodeId("\uD800")).toBeNull()
  })

  it("is canonical over any text: what decodes, encodes back to itself", () => {
    // Strings drawn near the alphabet, escapes and look-alikes among them.
    const candidates = [...ids, ...ids.map(encodeId), ...ids.map((id) => `%${id}`)]
    let decoded = 0
    for (const candidate of candidates) {
      const id = decodeId(candidate)
      if (id === null) continue
      decoded++
      expect(encodeId(id), JSON.stringify(candidate)).toBe(candidate)
    }
    // The table reaches both answers, not only refusals.
    expect(decoded).toBeGreaterThan(ids.length)
    expect(decoded).toBeLessThan(candidates.length)
  })
})
