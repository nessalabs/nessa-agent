import { describe, expect, it } from "vitest"
import { REVEAL_CHUNK, revealChunks } from "./reveal-text"

describe("splitting a line into reveal chunks", () => {
  it("reassembles into exactly the line it was given", () => {
    const line = "Welcome to Nessa"
    expect(revealChunks(line).join("")).toBe(line)
    expect(revealChunks(line, 5).join("")).toBe(line)
  })

  it("keeps whitespace inside the chunks rather than splitting it out", () => {
    // A reveal that drops or moves a space changes the sentence.
    expect(revealChunks("ab cd", 2)).toEqual(["ab", " c", "d"])
  })

  it("gives the remainder its own shorter chunk", () => {
    expect(revealChunks("abcde", 2)).toEqual(["ab", "cd", "e"])
  })

  it("does not tear a character outside the basic plane in half", () => {
    // Counted in code points: this is two characters, not four UTF-16 units.
    expect(revealChunks("😀😀", 1)).toEqual(["😀", "😀"])
  })

  it("has nothing to reveal for an empty line", () => {
    expect(revealChunks("")).toEqual([])
    expect(revealChunks("", 0)).toEqual([])
  })

  it("treats a size below one as the whole line at once", () => {
    expect(revealChunks("abc", 0)).toEqual(["abc"])
  })

  it("defaults to a size the eye reads rather than counts", () => {
    expect(REVEAL_CHUNK).toBe(2)
  })
})
