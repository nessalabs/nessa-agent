import { describe, expect, it } from "vitest"

import { bounds } from "../generated/product.js"
import {
  uploadedFileName,
  uploadedFilesProblem,
  uploadMediaType,
  validUploadedFileName,
} from "./attachment-validate.js"

const upload = (name: string) => [
  { digest: `sha256:${"a".repeat(64)}`, mimeType: "text/plain", size: 1, name },
]

const utf8 = new TextEncoder()
const byteLength = (value: string) => utf8.encode(value).byteLength

/** One more copy of `character` than fits in the published filename byte bound. */
function pastTheBound(character: string): string {
  const unit = byteLength(character)
  return character.repeat(Math.floor(bounds.maxUploadedFileNameBytes / unit) + 1)
}

describe("upload names and media types", () => {
  it("makes any name into one the upload contract takes", () => {
    for (const raw of [
      "notes.txt",
      "a/b\\c\u0007.txt",
      "   ",
      "\u0085",
      "é".repeat(200),
    ]) {
      const name = uploadedFileName(raw, "File")
      expect(validUploadedFileName(name)).toBe(true)
      expect(uploadedFilesProblem(upload(name))).toBeUndefined()
    }
    expect(uploadedFileName("notes.txt", "File")).toBe("notes.txt")
    expect(uploadedFileName("a/b\\c\u0007.txt", "File")).toBe("a_b_c_.txt")
    expect(uploadedFileName("   ", "File")).toBe("File")
    expect(uploadedFileName("\u00A0", "File")).toBe("File")
    expect(uploadedFileName("\u0085", "File")).toBe("_")
  })

  it("cuts a name on a code point so its UTF-8 length stays inside the published bound", () => {
    const limit = bounds.maxUploadedFileNameBytes
    for (const character of ["a", "é", "你", "😀"]) {
      const unit = byteLength(character)
      const over = pastTheBound(character)
      const name = uploadedFileName(over, "File")
      const kept = byteLength(name) / unit
      expect(Number.isInteger(kept)).toBe(true)
      expect(name).toBe(character.repeat(kept))
      expect(byteLength(name)).toBeLessThanOrEqual(limit)
      expect(byteLength(name) + unit).toBeGreaterThan(limit)
      expect(uploadedFilesProblem(upload(name))).toBeUndefined()
    }
    const exact = "b".repeat(limit)
    expect(uploadedFileName(exact, "File")).toBe(exact)
    expect(byteLength(uploadedFileName(`${exact}尾`, "File"))).toBe(limit)
  })

  it("refuses a name the upload contract rejects, and the normalized form is one it takes", () => {
    const limit = bounds.maxUploadedFileNameBytes
    const refused = [
      "",
      "   ",
      "\u00A0",
      "a/b",
      "a\\b",
      "\u0000notes",
      "\u001f",
      "\u007f",
      "\u0085",
      "\u009f",
      "\n",
      "a".repeat(limit + 1),
      pastTheBound("é"),
      pastTheBound("你"),
      pastTheBound("😀"),
    ]
    for (const raw of refused) {
      expect(validUploadedFileName(raw), JSON.stringify(raw)).toBe(false)
      expect(uploadedFilesProblem(upload(raw)), JSON.stringify(raw)).toBe(
        "invalid uploaded filename",
      )
      const name = uploadedFileName(raw, "File")
      expect(validUploadedFileName(name), JSON.stringify(raw)).toBe(true)
      expect(byteLength(name)).toBeLessThanOrEqual(limit)
      expect(uploadedFilesProblem(upload(name)), JSON.stringify(raw)).toBeUndefined()
    }
  })

  it("uploads under a bare media type, or octet-stream when there is none", () => {
    expect(uploadMediaType("Text/Plain; charset=utf-8")).toBe("text/plain")
    expect(uploadMediaType("")).toBe("application/octet-stream")
    expect(uploadMediaType("not a type")).toBe("application/octet-stream")
  })
})
