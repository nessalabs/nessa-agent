import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"
import AttachmentPreview from "./attachment-preview"
import type { FileAttachment } from "../../conversation"

function preview(name: string, size: number) {
  const file: FileAttachment = {
    type: "file",
    id: "preview",
    name,
    mimeType: "application/octet-stream",
    size,
    previewUrl: "blob:local-preview",
  }
  return renderToStaticMarkup(createElement(AttachmentPreview, { file }))
}

describe("attachment preview resource budget", () => {
  it.each(["rows.csv", "data.json", "notes.md", "source.ts"])(
    "keeps oversized %s downloadable without mounting the whole-file renderer",
    (name) => {
      const html = preview(name, 32 * 1024 + 1)
      expect(html).toContain("too large for an inline preview")
      expect(html).toContain('href="blob:local-preview"')
      expect(html).toContain(`download="${name}"`)
      expect(html).not.toContain('data-slot="file-preview-content"')
    },
  )

  it("still renders structured files within the budget", () => {
    expect(preview("rows.csv", 32 * 1024)).toContain('data-slot="file-preview-content"')
  })

  it("does not apply the text parsing budget to media", () => {
    expect(preview("photo.png", 20 * 1024 * 1024)).toContain(
      'data-slot="file-preview-image"',
    )
  })
})
