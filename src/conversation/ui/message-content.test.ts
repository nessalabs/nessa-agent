import * as React from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"

import { MessageContentView } from "./message-content"

describe("structural pasted parts", () => {
  it.each([
    "!",
    "\\",
    "`",
    "```ts\n",
    "[link](",
    "<!--",
    "**",
    "| cell |",
    "# Heading\n\n",
  ])("keeps pasted payloads out of Markdown following %j", (prefix) => {
    const html = renderToStaticMarkup(
      React.createElement(MessageContentView, {
        content: [
          { type: "text", text: prefix },
          { type: "pasted-text", id: "p", text: "secret payload" },
          { type: "text", text: "suffix" },
        ],
        onOpenPaste: () => {},
      }),
    )
    expect(html).toContain("Pasted text (14 chars)")
    expect(html).toContain("<button")
    expect(html).not.toContain("secret payload")
    expect(html).not.toContain("nessapaste")
    expect(html).not.toContain("<img")
  })
})

it("preserves bold and list structure spanning a pasted part", () => {
  const html = renderToStaticMarkup(
    React.createElement(MessageContentView, {
      content: [
        { type: "text", text: "- **before " },
        { type: "pasted-text", id: "p", text: "payload" },
        { type: "text", text: " after**" },
      ],
      onOpenPaste: () => {},
    }),
  )
  expect(html).toMatch(/<li[^>]*><strong>before <button/)
  expect(html).toContain(" after</strong></li>")
  expect(html).not.toContain("**")
})

it.each([
  ["`before ", " after`"],
  ["```ts\nbefore ", " after\n```"],
  ["<!-- before ", " after -->"],
  ["[link](https://example.com/", ")"],
  ["![image](https://example.com/", ")"],
  ["[before ", " after](https://example.com)"],
  ["$before ", " after$"],
])("retains an accessible pill inside %j and %j", (before, after) => {
  const html = renderToStaticMarkup(
    React.createElement(MessageContentView, {
      content: [
        { type: "text", text: before },
        { type: "pasted-text", id: "p", text: "secret payload" },
        { type: "text", text: after },
      ],
      onOpenPaste: () => {},
    }),
  )
  expect(html).toContain("Pasted text (14 chars)")
  expect(html).toContain("<button")
  expect(html).not.toContain("nessapaste")
  expect(html).not.toContain("secret payload")
  expect(html).not.toContain("<a ")
  expect(html).not.toContain("<img")
})

describe("a sent turn's images", () => {
  const digest = `sha256:${"ab".repeat(32)}`
  const local = {
    type: "file" as const,
    id: "f",
    name: "finder.png",
    mimeType: "image/png",
    size: 2048,
    previewUrl: "blob:finder",
    upload: {
      status: "stored" as const,
      image: { digest, mimeType: "image/png" as const, size: 2048 },
    },
  }
  const render = (content: React.ComponentProps<typeof MessageContentView>["content"]) =>
    renderToStaticMarkup(
      React.createElement(MessageContentView, { content, onOpenPaste: () => {} }),
    )

  it("paints an image-only turn from its local preview, with a name to reach it by", () => {
    const html = render([local])
    // The original as attached; the stored copy's digest is not something to show.
    expect(html).not.toContain(digest)
    expect(html).toContain('src="blob:finder"')
    expect(html).toContain('alt="finder.png"')
    expect(html).toContain('aria-label="1 image"')
    // No text, so no empty Markdown block ahead of the tile.
    expect(html).not.toMatch(/<(p|div)\b/)
  })

  it("paints a labelled placeholder for an image known only by reference", () => {
    const html = render([
      { type: "image-reference", digest, mimeType: "image/jpeg", size: 812 * 1024 },
    ])
    expect(html).toContain("JPEG image, 812 KiB")
    expect(html).toContain("data-image-reference")
    expect(html).not.toContain("<img")
    // The digest names bytes on the gateway; it is not something to show anybody.
    expect(html).not.toContain(digest)
  })

  it("paints a labelled tile, not a broken picture, for an original the webview cannot show", () => {
    const html = render([
      { ...local, name: "IMG_0042.CR3", mimeType: "image/x-canon-cr3" },
    ])
    expect(html).not.toContain("<img")
    expect(html).toContain("IMG_0042.CR3")
    expect(html).toContain('aria-label="1 image"')
  })

  it("keeps the text and shows every image after it, local and referenced alike", () => {
    const html = render([
      { type: "text", text: "compare **these**" },
      local,
      { type: "image-reference", digest, mimeType: "image/webp", size: 3 },
    ])
    expect(html).toContain("<strong>these</strong>")
    expect(html).toContain('aria-label="2 images"')
    expect(html.indexOf("<strong>")).toBeLessThan(html.indexOf("<ul"))
    expect(html).toContain("WebP image, 3 B")
  })

  it("adds nothing to a turn without images, and paints no tile for a file that is not one", () => {
    expect(render([{ type: "text", text: "plain" }])).not.toContain("<ul")
    const html = render([
      { type: "text", text: "see notes" },
      { ...local, name: "notes.pdf", mimeType: "application/pdf" },
    ])
    expect(html).not.toContain("<img")
    expect(html).not.toContain("nessa-message-images")
  })
})
