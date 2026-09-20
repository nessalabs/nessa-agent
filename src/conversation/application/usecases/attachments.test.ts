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
  changeUpload,
  failSend,
  removeFile,
  openConversation,
  setActive,
  setDraft,
  beginSend,
} from "./index"
import { imageRefusalMessage } from "./send-draft"

const file = (id: string, size = 1): FileAttachment => ({
  type: "file",
  id,
  size,
  name: `${id}.txt`,
  mimeType: "text/plain",
  previewUrl: "blob:test-file",
  upload: { status: "not-started" },
})

const DIGEST = `sha256:${"ab".repeat(32)}`
const image = (id: string, size = 3): FileAttachment => ({
  ...file(id, size),
  name: `${id}.png`,
  mimeType: "image/png",
})
const reference = (size = 3) => ({ digest: DIGEST, mimeType: "image/png" as const, size })
/** A draft holding one image the gateway has taken. */
function stored(id = "a") {
  let tabs = attachFiles(emptyLocalTabs(), [image(id)], "c0")
  tabs = changeUpload(tabs, { fileId: id, to: "uploading" })
  return changeUpload(tabs, { fileId: id, to: "stored", image: reference() })
}
const submission = {
  conversationId: "c0",
  executionId: "execution",
  actionId: "action",
  mode: "queued" as const,
}

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
  it("never submits or clears a file that cannot go, even if submission omits it", () => {
    const tabs = attachFiles(emptyLocalTabs(), [file("a")], "c0")
    expect(
      beginSend(tabs, {
        conversationId: "c0",
        executionId: "execution",
        actionId: "action",
        mode: "queued",
        content: [{ type: "text", text: "hello" }, file("a")],
      }),
    ).toBe(tabs)
    expect(
      beginSend(tabs, {
        conversationId: "c0",
        executionId: "execution",
        actionId: "action",
        mode: "queued",
        content: [{ type: "text", text: "hello" }],
      }),
    ).toBe(tabs)
    expect(
      contentText([
        { type: "text", text: "  hi " },
        file("a"),
        { type: "pasted-text", id: "p", text: " there\n" },
      ]),
    ).toBe("  hi  there\n")
  })
})

describe("a draft file's upload state", () => {
  it("attaches every file not started, whatever the caller claimed for it", () => {
    const claimed = {
      ...image("a"),
      upload: { status: "stored" as const, image: reference() },
    }
    const tabs = attachFiles(emptyLocalTabs(), [claimed], "c0")
    expect(tabs.conversations[0]!.draft).toEqual([image("a")])
  })

  it("moves one step at a time and ignores steps that do not follow", () => {
    const attached = attachFiles(emptyLocalTabs(), [image("a")], "c0")
    // Nothing is in flight, so there is nothing to finish, fail, or replace.
    expect(
      changeUpload(attached, { fileId: "a", to: "stored", image: reference() }),
    ).toBe(attached)
    expect(
      changeUpload(attached, { fileId: "a", to: "failed", reason: "rejected" }),
    ).toBe(attached)
    expect(changeUpload(attached, { fileId: "a", to: "not-started" })).toBe(attached)
    const uploading = changeUpload(attached, { fileId: "a", to: "uploading" })
    expect(changeUpload(uploading, { fileId: "a", to: "uploading" })).toBe(uploading)
    const done = changeUpload(uploading, {
      fileId: "a",
      to: "stored",
      image: reference(),
    })
    expect(done.conversations[0]!.draft).toEqual([
      { ...image("a"), upload: { status: "stored", image: reference() } },
    ])
    // A second result for the same upload describes nothing on screen.
    expect(changeUpload(done, { fileId: "a", to: "failed", reason: "unavailable" })).toBe(
      done,
    )
    expect(changeUpload(done, { fileId: "a", to: "not-started" })).toBe(done)
  })

  it("retries only a failure, and keeps why it failed until then", () => {
    const uploading = changeUpload(attachFiles(emptyLocalTabs(), [image("a")], "c0"), {
      fileId: "a",
      to: "uploading",
    })
    const failed = changeUpload(uploading, {
      fileId: "a",
      to: "failed",
      reason: "unavailable",
    })
    expect(failed.conversations[0]!.draft).toEqual([
      { ...image("a"), upload: { status: "failed", reason: "unavailable" } },
    ])
    const again = changeUpload(failed, { fileId: "a", to: "not-started" })
    expect(again.conversations[0]!.draft).toEqual([image("a")])
  })

  it("stores the gateway's reference whole, however little it resembles the file", () => {
    // A 9 MB HEIC goes up; a 400 KB JPEG under another digest is what is stored.
    const heic = {
      ...image("a", 9_000_000),
      name: "holiday.heic",
      mimeType: "image/heic",
    }
    const stored = {
      digest: `sha256:${"cd".repeat(32)}`,
      mimeType: "image/jpeg" as const,
      size: 400_000,
    }
    const uploading = changeUpload(attachFiles(emptyLocalTabs(), [heic], "c0"), {
      fileId: "a",
      to: "uploading",
    })
    const done = changeUpload(uploading, { fileId: "a", to: "stored", image: stored })
    // The file still describes the original, which is what is previewed.
    expect(done.conversations[0]!.draft).toEqual([
      { ...heic, upload: { status: "stored", image: stored } },
    ])
  })

  it.each([
    ["something that is not a digest", { ...reference(), digest: "sha256:ABC" }],
    [
      "an encoding no message names",
      { ...reference(), mimeType: "image/heic" as "image/png" },
    ],
    ["nothing at all", reference(0)],
    ["a fractional size", reference(1.5)],
  ])("fails, rather than stores, a reference to %s", (_name, claimed) => {
    const uploading = changeUpload(attachFiles(emptyLocalTabs(), [image("a")], "c0"), {
      fileId: "a",
      to: "uploading",
    })
    const result = changeUpload(uploading, { fileId: "a", to: "stored", image: claimed })
    expect(result.conversations[0]!.draft).toEqual([
      { ...image("a"), upload: { status: "failed", reason: "rejected" } },
    ])
  })

  it("never stores a reference on a file that is not an image", () => {
    const uploading = changeUpload(attachFiles(emptyLocalTabs(), [file("a", 3)], "c0"), {
      fileId: "a",
      to: "uploading",
    })
    const result = changeUpload(uploading, {
      fileId: "a",
      to: "stored",
      image: reference(),
    })
    expect(result.conversations[0]!.draft[0]).toMatchObject({
      upload: { status: "failed", reason: "rejected" },
    })
  })

  it("keeps the gateway's verdict on an image as the reason it failed", () => {
    for (const reason of ["unsupported-image", "too-large"] as const) {
      const uploading = changeUpload(attachFiles(emptyLocalTabs(), [image("a")], "c0"), {
        fileId: "a",
        to: "uploading",
      })
      const failed = changeUpload(uploading, { fileId: "a", to: "failed", reason })
      expect(failed.conversations[0]!.draft[0]).toMatchObject({
        upload: { status: "failed", reason },
      })
    }
  })

  it("keeps the store's upload state when the composer rewrites the draft around it", () => {
    // The composer rebuilds the draft from its last render, where the upload
    // was still in flight; by the keystroke, it had finished.
    const stale = { ...image("a"), upload: { status: "uploading" as const } }
    const typed = setDraft(stored(), { draft: [{ type: "text", text: "hi" }, stale] })
    expect(typed.conversations[0]!.draft).toEqual([
      { type: "text", text: "hi" },
      { ...image("a"), upload: { status: "stored", image: reference() } },
    ])
    // And a file the draft never held cannot arrive already claiming to be stored.
    const claimed = {
      ...image("b"),
      upload: { status: "stored" as const, image: reference() },
    }
    const smuggled = setDraft(emptyLocalTabs(), { draft: [claimed] })
    expect(smuggled.conversations[0]!.draft).toEqual([image("b")])
  })

  it("does not bring back a file removed while its upload was in flight", () => {
    const uploading = changeUpload(attachFiles(emptyLocalTabs(), [image("a")], "c0"), {
      fileId: "a",
      to: "uploading",
    })
    const removed = removeFile(uploading, "a")
    expect(removed.conversations[0]!.draft).toEqual([])
    for (const late of [
      { fileId: "a", to: "stored" as const, image: reference() },
      { fileId: "a", to: "failed" as const, reason: "unavailable" as const },
      { fileId: "a", to: "not-started" as const },
    ])
      expect(changeUpload(removed, late)).toBe(removed)
  })

  it("finds the file in whichever tab holds it, not only the active one", () => {
    const tabs = openConversation(attachFiles(emptyLocalTabs(), [image("a")], "c0"))
    expect(tabs.activeId).not.toBe("c0")
    const uploading = changeUpload(tabs, { fileId: "a", to: "uploading" })
    expect(uploading.conversations[0]!.draft[0]).toMatchObject({
      upload: { status: "uploading" },
    })
  })
})

describe("sending a draft that holds images", () => {
  it("turns stored images and text into one turn and empties the draft", () => {
    const tabs = stored()
    const content = [
      { type: "text" as const, text: "look" },
      ...tabs.conversations[0]!.draft,
    ]
    const sent = beginSend(tabs, { ...submission, content })
    expect(sent.conversations[0]!.draft).toEqual([])
    expect(sent.conversations[0]!.turns).toMatchObject([
      { from: "user", receipt: "sending", content },
    ])
    expect(sent.conversations[0]!.title).toBe("look")
  })

  it("sends a message of images alone, titled by the first image", () => {
    const tabs = stored("finder")
    const sent = beginSend(tabs, { ...submission, content: tabs.conversations[0]!.draft })
    expect(sent.conversations[0]!.turns).toHaveLength(1)
    expect(sent.conversations[0]!.title).toBe("finder.png")
    expect(sent.conversations[0]!.phase).toBe("thinking")
  })

  it("begins nothing while any file is unsent, failed, or left out of the message", () => {
    const attached = attachFiles(emptyLocalTabs(), [image("a")], "c0")
    const uploading = changeUpload(attached, { fileId: "a", to: "uploading" })
    const failed = changeUpload(uploading, {
      fileId: "a",
      to: "failed",
      reason: "rejected",
    })
    for (const tabs of [attached, uploading, failed]) {
      const content = [
        { type: "text" as const, text: "hi" },
        ...tabs.conversations[0]!.draft,
      ]
      expect(beginSend(tabs, { ...submission, content })).toBe(tabs)
    }
    const ready = stored()
    expect(
      beginSend(ready, { ...submission, content: [{ type: "text", text: "hi" }] }),
    ).toBe(ready)
    expect(beginSend(ready, { ...submission, content: [] })).toBe(ready)
  })

  it("puts a refused message's images back in the draft, still stored", () => {
    const tabs = stored()
    const content = tabs.conversations[0]!.draft
    const sent = beginSend(tabs, { ...submission, content })
    const refused = failSend(sent, "c0", "execution", "This agent takes no images", false)
    expect(refused.conversations[0]!.draft).toEqual(content)
    expect(refused.conversations[0]!.turns[0]).toMatchObject({ receipt: "failed" })
    // An uncertain failure keeps the turn for retry and leaves the draft alone.
    const lost = failSend(sent, "c0", "execution", "connection lost")
    expect(lost.conversations[0]!.draft).toEqual([])
    expect(lost.conversations[0]!.turns[0]).toMatchObject({ receipt: "unknown", content })
  })

  it("says something different, and useful, for each refusal", () => {
    const messages = [
      imageRefusalMessage({ kind: "unsupported-file", name: "notes.pdf" }),
      imageRefusalMessage({ kind: "upload-failed", name: "a.png" }),
      imageRefusalMessage({ kind: "upload-in-flight" }),
      imageRefusalMessage({ kind: "too-many-images" }),
      imageRefusalMessage({ kind: "images-too-large" }),
    ]
    expect(new Set(messages).size).toBe(messages.length)
    expect(messages[0]).toContain("notes.pdf")
    expect(messages[1]).toContain("a.png")
  })
})
