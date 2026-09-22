import * as React from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"

import { messageFiles, referencedContent } from "../model"
import { MessageContentView } from "./message-content"
import { MessageImages } from "./message-images"

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
    path: null,
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
    expect(html).toContain('aria-label="1 attachment"')
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
    expect(html).toContain('aria-label="1 attachment"')
  })

  it("keeps the text and shows every image after it, local and referenced alike", () => {
    const html = render([
      { type: "text", text: "compare **these**" },
      local,
      { type: "image-reference", digest, mimeType: "image/webp", size: 3 },
    ])
    expect(html).toContain("<strong>these</strong>")
    expect(html).toContain('aria-label="2 attachments"')
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

  it("shows a file the turn pointed at, by name, with its path to hover", () => {
    // Attached in this window: the tile is named by the file and titled by
    // where it is, because the path is what the agent was given and what a
    // permission prompt will quote.
    const html = renderToStaticMarkup(
      React.createElement(MessageImages, {
        content: [
          { type: "text", text: "read this" },
          {
            ...local,
            id: "d",
            name: "report.pdf",
            mimeType: "application/pdf",
            previewUrl: "",
            path: "/Users/ada/notes/report.pdf",
          },
        ],
      }),
    )
    expect(html).toContain('aria-label="1 attachment"')
    expect(html).toContain('title="/Users/ada/notes/report.pdf"')
    expect(html).toContain("report.pdf<")
    // Nothing was ever held for it, so nothing is painted and nothing fetched.
    expect(html).not.toContain("<img")
    expect(html).not.toContain("blob:")
  })

  it("keeps a turn's files after a reload, when only the gateway remembers them", () => {
    // The regression this covers: the view carries the paths a turn named, and
    // a turn rebuilt from it used to drop them, so a reload silently emptied
    // the attachment row while the agent had been given those files.
    const restored = referencedContent(
      "read these",
      [],
      [
        { path: "/Users/ada/report.pdf" },
        { path: "/Users/ada/notes/summary (final).md" },
      ],
    )
    const html = renderToStaticMarkup(
      React.createElement(MessageImages, { content: restored }),
    )
    expect(html).toContain('aria-label="2 attachments"')
    expect(html).toContain('title="/Users/ada/report.pdf"')
    expect(html).toContain('title="/Users/ada/notes/summary (final).md"')
    // Labelled by name, not by the whole path, which would not fit.
    expect(html).toContain("report.pdf<")
    expect(html).toContain("summary (final).md<")
    // And a retry of that turn still names exactly the same paths.
    expect(messageFiles(restored)).toEqual([
      { path: "/Users/ada/report.pdf" },
      { path: "/Users/ada/notes/summary (final).md" },
    ])
  })
})
