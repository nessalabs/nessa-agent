import { describe, expect, it } from "vitest"
import {
  contentText,
  MAX_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENTS,
  type FileAttachment,
} from "../../model"
import { emptyLocalTabs } from "../local-tabs"
import {
  attachFiles,
  removeFile,
  openConversation,
  setActive,
  setDraft,
  beginSend,
} from "./index"

const file = (id: string, size = 1): FileAttachment => ({
  type: "file",
  id,
  size,
  name: `${id}.txt`,
  mimeType: "text/plain",
  previewUrl: "blob:test-file",
})

describe("draft file previews", () => {
  it("retains files in the originating tab and removes only the requested active file", () => {
    const tabs = openConversation(
      attachFiles(emptyLocalTabs(), [file("a"), file("b")], "c0"),
    )
    expect(removeFile(tabs, "a")).toBe(tabs)
    const active = setActive(tabs, "c0")
    const next = removeFile(active, "a")
    expect(next.conversations[0]!.draft).toEqual([file("b")])
    expect(next.conversations[1]!.draft).toEqual([])
    expect(attachFiles(tabs, [file("c")], "missing")).toBe(tabs)
  })
  it("enforces per-file, total-size and count limits atomically", () => {
    const tabs = emptyLocalTabs()
    expect(attachFiles(tabs, [file("a", MAX_ATTACHMENT_BYTES + 1)], "c0")).toBe(tabs)
    const full = attachFiles(
      tabs,
      [
        file("a", MAX_ATTACHMENT_BYTES),
        file("b", MAX_ATTACHMENT_BYTES),
        file("c", MAX_DRAFT_ATTACHMENT_BYTES - 2 * MAX_ATTACHMENT_BYTES),
      ],
      "c0",
    )
    expect(full.conversations[0]!.draft).toHaveLength(3)
    expect(attachFiles(full, [file("d")], "c0")).toBe(full)
    const count = attachFiles(
      tabs,
      Array.from({ length: MAX_DRAFT_ATTACHMENTS }, (_, i) => file(`${i}`, 0)),
      "c0",
    )
    expect(count.conversations[0]!.draft).toHaveLength(MAX_DRAFT_ATTACHMENTS)
    expect(attachFiles(count, [file("extra", 0)], "c0")).toBe(count)
    expect(attachFiles(tabs, [file("a"), file("a")], "c0")).toBe(tabs)
    expect(attachFiles(tabs, [file("a", -1)], "c0")).toBe(tabs)
    expect(setDraft(tabs, { draft: [file("a", MAX_ATTACHMENT_BYTES + 1)] })).toBe(tabs)
  })
  it("never submits or clears previews, even if submission omits the current files", () => {
    const tabs = attachFiles(emptyLocalTabs(), [file("a")], "c0")
    expect(
      beginSend(tabs, { content: [{ type: "text", text: "hello" }, file("a")] }),
    ).toBe(tabs)
    expect(beginSend(tabs, { content: [{ type: "text", text: "hello" }] })).toBe(tabs)
    expect(
      contentText([
        { type: "text", text: "  hi " },
        file("a"),
        { type: "pasted-text", id: "p", text: " there\n" },
      ]),
    ).toBe("  hi  there\n")
  })
})
