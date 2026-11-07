import { describe, expect, it } from "vitest"

import {
  uploadedFileName,
  uploadedFilesProblem,
  uploadMediaType,
  validUploadedFileName,
} from "./attachment-validate.js"

const upload = (name: string) => [
  { digest: `sha256:${"a".repeat(64)}`, mimeType: "text/plain", size: 1, name },
]

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
  })

  it("uploads under a bare media type, or octet-stream when there is none", () => {
    expect(uploadMediaType("Text/Plain; charset=utf-8")).toBe("text/plain")
    expect(uploadMediaType("")).toBe("application/octet-stream")
    expect(uploadMediaType("not a type")).toBe("application/octet-stream")
  })
})
