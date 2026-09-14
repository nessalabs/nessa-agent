import { describe, expect, it } from "vitest"
import { contentText, type MessageContent } from "../model"
import { fromEditor, toEditor } from "./composer-content"

describe("composer content", () => {
  it("round-trips ordered text and pasted payloads without interpreting their Markdown", () => {
    const content: MessageContent = [
      { type: "text", text: "Compare **these**:\n" },
      { type: "pasted-text", id: "a", text: "  <tag>\n```ts\nconst a = 1\n```\n\n" },
      { type: "text", text: "\nwith " },
      { type: "pasted-text", id: "b", text: "&nbsp;\tsecond\n" },
    ]
    expect(fromEditor(toEditor(content))).toEqual(content)
    expect(toEditor(content).text).toBe(contentText(content))
  })
  it("restores a pasted-only draft as a chip, not an empty editor", () => {
    const content: MessageContent = [{ type: "pasted-text", id: "p", text: "payload" }]
    expect(toEditor(content).parts).toEqual([
      {
        type: "chip",
        chip: {
          id: "p",
          kind: "pasted-text",
          label: "Pasted text (7 chars)",
          textValue: "payload",
        },
      },
    ])
    expect(fromEditor(toEditor([]))).toEqual([])
  })
})
