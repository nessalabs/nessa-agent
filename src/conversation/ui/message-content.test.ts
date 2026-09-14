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
