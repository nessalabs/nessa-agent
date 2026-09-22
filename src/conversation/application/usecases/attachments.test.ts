import { describe, expect, it } from "vitest"
import {
  isImageFile,
  linkedFile,
  MAX_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENTS,
  type Conversation,
  type ConversationCapabilities,
  type FileAttachment,
} from "../../model"
import { emptyLocalTabs } from "../local-tabs"
import {
  attachFiles,
  changeUpload,
  failSend,
  forgetStoredUploads,
  removeFile,
  openConversation,
  setActive,
  setDraft,
  beginSend,
  uploadFailureSummary,
  uploadFailureText,
  worthRetrying,
} from "./index"
import {
  declineReason,
  draftMessage,
  imageRefusalMessage,
  refusalReleasesImages,
  submissionRefusalMessage,
} from "./send-draft"
import { controlFailureMessage } from "./control-failure"
import { boundSentPreviews } from "./release-uploads"

const file = (id: string, size = 1): FileAttachment => ({
  type: "file",
  id,
  size,
  name: `${id}.txt`,
  mimeType: "text/plain",
  previewUrl: "blob:test-file",
  upload: { status: "not-started" },
  path: null,
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
const capabilities: ConversationCapabilities = {
  queue: true,
  steer: false,
  resume: false,
  permissions: false,
  imageInput: true,
  agentFeatures: {
    permissionDenial: "unknown",
    nativeHookSuppression: "unknown",
    compactionReporting: "unsupported_not_implemented",
    modelSwitchReporting: "unsupported_not_implemented",
    permissionDeferral: "unsupported_not_implemented",
    elicitationForwarding: "unknown",
    preToolPolicy: "unsupported_not_implemented",
    policyEndTurn: "unsupported_not_implemented",
    policyCloseSession: "unsupported_not_implemented",
    incomingElicitation: "unsupported_not_implemented",
  },
}
/** A view has arrived for this conversation, and it said the agent takes images. */
const remote = {
  running: false,
  permissions: [],
  tools: [],
  pending: [],
  capabilities,
  queueComplete: true,
  truncated: false,
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
    // A draft's files are the store's: `setDraft` is no way to bring one in.
    expect(
      setDraft(tabs, { draft: [file("a", MAX_ATTACHMENT_BYTES + 1)] }).conversations[0]!
        .draft,
    ).toEqual([])
  })
  it("never submits or clears a file that cannot go, even if submission omits it", () => {
    const tabs = attachFiles(emptyLocalTabs(), [file("a")], "c0")
    for (const content of [
      [{ type: "text" as const, text: "hello" }, file("a")],
      [{ type: "text" as const, text: "hello" }],
    ])
      expect(beginSend(tabs, { ...submission, content })).toBe(tabs)
  })
})

describe("a draft file's upload state", () => {
  it("attaches every file not started, whatever the caller claimed for it", () => {
    const claimed = {
      ...image("a"),
      upload: { status: "stored" as const, image: reference() },
      path: null,
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
      path: null,
    })
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
  })

  it("never lets the composer's last render put back a file that was removed", () => {
    const held = stored()
    const rendered = held.conversations[0]!.draft
    const removed = removeFile(held, "a")
    const typed = setDraft(removed, {
      draft: [{ type: "text", text: "hi" }, ...rendered],
    })
    expect(typed.conversations[0]!.draft).toEqual([{ type: "text", text: "hi" }])
  })

  it("never lets the composer's last render put back a file that was sent", () => {
    const held = stored()
    const rendered = held.conversations[0]!.draft
    const sent = beginSend(held, { ...submission, content: rendered })
    expect(sent.conversations[0]!.draft).toEqual([])
    const typed = setDraft(sent, { draft: [{ type: "text", text: "next" }, ...rendered] })
    // In the turn, and only there: the same image is not also drafted again.
    expect(typed.conversations[0]!.draft).toEqual([{ type: "text", text: "next" }])
    expect(typed.conversations[0]!.turns[0]).toMatchObject({ content: rendered })
  })

  it("keeps a file attached since the composer's last render, which never saw it", () => {
    const before = emptyLocalTabs()
    const attached = attachFiles(before, [image("new")], "c0")
    const typed = setDraft(attached, { draft: [{ type: "text", text: "hi" }] })
    expect(typed.conversations[0]!.draft).toEqual([
      { type: "text", text: "hi" },
      image("new"),
    ])
  })

  it("cannot be used to bring in a file, stored or otherwise", () => {
    const claimed = {
      ...image("b"),
      upload: { status: "stored" as const, image: reference() },
      path: null,
    }
    expect(
      setDraft(emptyLocalTabs(), { draft: [claimed] }).conversations[0]!.draft,
    ).toEqual([])
  })

  it("declares a file the browser gave no type as an image when its extension says so", () => {
    const raw = {
      ...file("raw", 3),
      name: "IMG_0042.CR3",
      mimeType: "application/octet-stream",
    }
    const heic = { ...file("heic", 3), name: "holiday.heic", mimeType: "" }
    const notes = { ...file("notes", 3), name: "notes.bin", mimeType: "" }
    const tabs = attachFiles(emptyLocalTabs(), [raw, heic, notes], "c0")
    expect(
      tabs.conversations[0]!.draft.map((part) => part.type === "file" && part.mimeType),
    ).toEqual(["image/x-canon-cr3", "image/heic", "application/octet-stream"])
    // Which makes the RAW file sendable once the gateway has stored it.
    const uploading = changeUpload(tabs, { fileId: "raw", to: "uploading" })
    const done = changeUpload(uploading, {
      fileId: "raw",
      to: "stored",
      image: reference(),
    })
    expect(done.conversations[0]!.draft[0]).toMatchObject({
      upload: { status: "stored", image: reference() },
      path: null,
    })
  })

  it("forgets what the gateway held once the conversation has been closed there", () => {
    let tabs = attachFiles(stored("kept"), [image("flying"), image("broken")], "c0")
    tabs = changeUpload(tabs, { fileId: "flying", to: "uploading" })
    tabs = changeUpload(tabs, { fileId: "broken", to: "uploading" })
    tabs = changeUpload(tabs, { fileId: "broken", to: "failed", reason: "rejected" })
    const released = forgetStoredUploads(tabs, "c0")
    expect(
      released.conversations[0]!.draft.map((part) => part.type === "file" && part.upload),
    ).toEqual([
      // Its hold is gone, so it is an image that has not been uploaded.
      { status: "not-started" },
      // Still in flight, and a failure is still a failure.
      { status: "uploading" },
      { status: "failed", reason: "rejected" },
    ])
    // Nothing stored, nothing to forget; and another tab's draft is not touched.
    expect(forgetStoredUploads(released, "c0")).toBe(released)
    expect(forgetStoredUploads(tabs, "missing")).toBe(tabs)
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
      path: null,
    })
  })
})

/** A send refused before admission: the message was not taken, so the draft is back. */
const refusedSend = { kind: "refused", reupload: false } as const

describe("why an upload failed, in words", () => {
  const reasons = [
    "unreadable",
    "unsupported-image",
    "too-large",
    "image-input-unsupported",
    "busy",
    "interrupted",
    "unavailable",
    "rejected",
  ] as const

  it("says something different for each reason, and honest about which", () => {
    const texts = reasons.map(uploadFailureText)
    expect(new Set(texts).size).toBe(reasons.length)
    // Not "refused" and not "could not be reached": each names what happened.
    expect(uploadFailureText("busy")).toMatch(/busy with other uploads/)
    expect(uploadFailureText("interrupted")).toMatch(/cut off or timed out/)
    expect(uploadFailureText("unsupported-image")).toMatch(/could not read this image/)
    // Never a byte or pixel limit: those are the gateway's, and per model.
    for (const text of texts) expect(text).not.toMatch(/\d\s?(MB|MiB|px)/)
  })

  it("has a short whole sentence for each reason, for a notification with room for one", () => {
    const summaries = reasons.map(uploadFailureSummary)
    expect(new Set(summaries).size).toBe(reasons.length)
    for (const summary of summaries) {
      // A sentence, short enough for one line, and never a byte or pixel limit.
      expect(summary).toMatch(/^[A-Z].*\.$/)
      expect(summary.length).toBeLessThanOrEqual(48)
      expect(summary).not.toMatch(/\d\s?(MB|MiB|px)/)
    }
    expect(uploadFailureSummary("unavailable")).toBe("Couldn't reach the gateway.")
  })

  it("offers a retry for everything but a verdict on the image or the agent", () => {
    expect(reasons.filter((reason) => !worthRetrying(reason))).toEqual([
      "unsupported-image",
      "too-large",
      "image-input-unsupported",
    ])
  })
})

describe("why a draft is declined before anything is sent", () => {
  const hello = [{ type: "text" as const, text: "hello" }]
  /** The conversation `stored()` built, with the gateway's answer about images. */
  const withImages = (imageInput?: boolean) => {
    const conv = stored().conversations[0]!
    return imageInput === undefined
      ? conv
      : { ...conv, remote: { ...remote, capabilities: { ...capabilities, imageInput } } }
  }

  it("takes a draft that can go, whatever the caller left out of its prose", () => {
    expect(declineReason(withImages(true), { content: hello }, true)).toBeNull()
    // Text alone needs no view: whether the agent takes images is not asked.
    expect(
      declineReason(emptyLocalTabs().conversations[0]!, { content: hello }, true),
    ).toBeNull()
  })

  type DeclineCase = [Conversation, Parameters<typeof declineReason>[1]]
  it.each<[string, DeclineCase, string]>([
    [
      "the caller has no session",
      [withImages(true), { content: hello, connected: false }],
      "not-connected",
    ],
    [
      "the caller named a file this draft does not hold",
      [withImages(true), { content: [...hello, image("stranger")] }],
      "unknown-attachment",
    ],
    [
      "a file cannot go as an image",
      [
        attachFiles(emptyLocalTabs(), [file("notes")], "c0").conversations[0]!,
        { content: hello },
      ],
      "unsupported-file",
    ],
    [
      "there is neither text nor an image",
      [emptyLocalTabs().conversations[0]!, { content: [] }],
      "empty-draft",
    ],
    [
      "the text is over what one message carries",
      [
        emptyLocalTabs().conversations[0]!,
        { content: [{ type: "text" as const, text: "x".repeat(8193) }] },
      ],
      "message-too-large",
    ],
    [
      "nobody has said yet whether the agent takes images",
      [withImages(), { content: hello }],
      "image-input-unknown",
    ],
    [
      "the agent takes no images",
      [withImages(false), { content: hello }],
      "image-input-unsupported",
    ],
  ])("declines because %s", (_name, args, kind) => {
    const decline = declineReason(...args, true)
    expect(decline).toMatchObject({ kind })
    // Every decline says something but the empty draft: there is nothing to
    // tell about nothing, and the composer shows the rest above itself.
    expect(Boolean(decline?.message)).toBe(kind !== "empty-draft")
  })

  it("asks for a fresh view only when the decline is 'not known yet'", () => {
    expect(declineReason(withImages(), { content: hello }, true)?.askAgain).toBe(true)
    for (const conv of [withImages(false), withImages(true)])
      expect(declineReason(conv, { content: [] }, true)?.askAgain).toBeUndefined()
  })

  it("does not send a browser to a picker that would refuse the file again", () => {
    // A file nothing can say the location of. In the app there is a route out
    // of that and the sentence names it; in a browser there is not, and naming
    // it anyway would hand back another file with no path and the same refusal
    // — while the notice over the composer said the opposite at the same time.
    const conv = attachFiles(emptyLocalTabs(), [file("notes")], "c0").conversations[0]!
    const inApp = declineReason(conv, { content: hello }, true)
    const inBrowser = declineReason(conv, { content: hello }, false)
    expect(inApp?.kind).toBe("unsupported-file")
    expect(inBrowser?.kind).toBe("unsupported-file")
    expect(inApp?.message).toContain("choose it with +")
    expect(inBrowser?.message).not.toContain("+")
    expect(inBrowser?.message).toContain("Nessa app")
    // Both still name the file and both still keep the draft.
    for (const decline of [inApp, inBrowser]) expect(decline?.message).toContain("notes")
  })

  it("sends the draft's own files, never the caller's copy of one", () => {
    const conv = withImages(true)
    const stale = { ...image("a"), upload: { status: "uploading" as const } }
    // The caller's prose is kept and its file part discarded: the draft's file,
    // which is the one with the stored reference, is what the message carries.
    expect(draftMessage(conv, [...hello, stale])).toEqual([...hello, ...conv.draft])
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
    const refused = failSend(
      sent,
      "c0",
      "execution",
      "This agent takes no images",
      refusedSend,
    )
    expect(refused.conversations[0]!.draft).toEqual(content)
    expect(refused.conversations[0]!.turns[0]).toMatchObject({ receipt: "failed" })
    // An uncertain failure keeps the turn for retry and leaves the draft alone.
    const lost = failSend(sent, "c0", "execution", "connection lost", {
      kind: "uncertain",
    })
    expect(lost.conversations[0]!.draft).toEqual([])
    expect(lost.conversations[0]!.turns[0]).toMatchObject({ receipt: "unknown", content })
  })

  it("puts a message whose images the gateway no longer holds back as images to upload again", () => {
    const tabs = stored()
    const content = tabs.conversations[0]!.draft
    const sent = beginSend(tabs, { ...submission, content })
    const refused = failSend(sent, "c0", "execution", "released", {
      kind: "refused",
      reupload: true,
    })
    // The same file, with the dead reference gone: the panel uploads it again.
    expect(refused.conversations[0]!.draft).toEqual([image("a")])
    // The turn keeps what was actually attempted.
    expect(refused.conversations[0]!.turns[0]).toMatchObject({
      receipt: "failed",
      content,
    })
    // An uncertain failure recovers nothing, so there is nothing to reset.
    expect(
      failSend(sent, "c0", "execution", "lost", { kind: "uncertain" }).conversations[0]!
        .draft,
    ).toEqual([])
  })

  it("says something different, and specific, for each refusal the gateway can give", () => {
    const reasons = [
      "image-input-unsupported",
      "attachment-not-found",
      "attachment-unavailable",
      "conversation-not-found",
      "conversation-capacity",
    ] as const
    const messages = reasons.map((reason) => submissionRefusalMessage(reason))
    expect(new Set(messages).size).toBe(reasons.length)
    for (const message of messages) expect(message).toMatch(/back in the draft/)
    expect(submissionRefusalMessage("attachment-not-found")).toMatch(/uploading again/)
    expect(reasons.filter(refusalReleasesImages)).toEqual([
      "attachment-not-found",
      "attachment-unavailable",
    ])
    // The client's own message for these says more than a sentence here could.
    expect(submissionRefusalMessage("agent-not-configured")).toBeUndefined()
    expect(submissionRefusalMessage("agent-startup-deadline")).toBeUndefined()
    expect(submissionRefusalMessage("invalid-request")).toBeUndefined()
  })

  const OUTCOMES = ["refused", "applied", "unknown"] as const
  /** Every reason the vocabulary has, and the absence of one, which is also a case. */
  const REASONS = [
    "image-input-unsupported",
    "attachment-not-found",
    "attachment-unavailable",
    "attachment-cleanup-unavailable",
    "conversation-not-found",
    "conversation-capacity",
    "agent-not-configured",
    "agent-startup-deadline",
    "invalid-request",
    undefined,
  ] as const

  it("leaves a control's news to the control, and a message's to the message", () => {
    // Two commands, two kinds of news, one vocabulary. A close that could not
    // let go of its files is the control's to say, whichever way the rest of
    // the command ended, and is nothing a send can be refused with.
    for (const outcome of OUTCOMES)
      expect(controlFailureMessage("attachment-cleanup-unavailable", outcome)).toMatch(
        /could not release the images/,
      )
    expect(submissionRefusalMessage("attachment-cleanup-unavailable")).toBeUndefined()
  })

  it("says what became of a control whenever the gateway was certain, whatever the reason", () => {
    // The client has one sentence for every failed control and it claims the
    // outcome is unknown. Which of the three sentences the panel shows is the
    // outcome's to decide, never the reason's — a review the gateway left
    // pending is refused under any code, including one with no word here.
    for (const reason of REASONS) {
      if (reason === "attachment-cleanup-unavailable") continue
      expect(
        controlFailureMessage(reason, "refused"),
        `${reason ?? "no reason"} refused`,
      ).toMatch(/nothing was done/)
      expect(
        controlFailureMessage(reason, "applied"),
        `${reason ?? "no reason"} applied`,
      ).toMatch(/nothing to answer again/)
      // Only here is the client's own sentence still the honest one.
      expect(
        controlFailureMessage(reason, "unknown"),
        `${reason ?? "no reason"} unknown`,
      ).toBeUndefined()
    }
  })

  it("adds what the reason is worth to a refusal without needing one", () => {
    // The reason shapes the sentence and never licenses it, so the ones that
    // have something specific to add say it, and the rest share a sentence.
    const named = ["agent-startup-deadline", "agent-not-configured"] as const
    for (const reason of named)
      expect(controlFailureMessage(reason, "refused")).not.toBe(
        controlFailureMessage(undefined, "refused"),
      )
    expect(controlFailureMessage("agent-startup-deadline", "refused")).toMatch(
      /still starting/,
    )
    expect(controlFailureMessage("invalid-request", "refused")).toBe(
      controlFailureMessage(undefined, "refused"),
    )
  })

  it("blames this conversation's agent and not the gateway's, when only one is missing", () => {
    // These two codes mean different things and the server split them so the
    // panel could say so. An operator with Claude and Codex configured who
    // drops `codex` and restarts still has a working gateway; pressing Close on
    // a Codex tab must not send them to a config.json that already configures
    // an agent. Asserted on the content, because a test that only checks the
    // two sentences differ stays green through exactly this drift.
    const one = controlFailureMessage("agent-not-configured", "refused")
    expect(one).toMatch(/this conversation runs on/i)
    expect(one).not.toMatch(/(has no|not set up to run|no agent)/i)

    const none = controlFailureMessage("conversations-not-configured", "refused")
    expect(none).toMatch(/not set up to run conversations/i)
    expect(none).not.toBe(one)
  })

  it("offers no remedy for an agent this build cannot open, because there is none to offer", () => {
    // The agents Nessa can drive are compiled in, so no edit to config.json
    // adds one — and naming an agent there that has no adapter stops the
    // gateway starting at all. A panel that says "configure that agent" turns
    // one stranded conversation into no gateway.
    const message = controlFailureMessage("agent-unsupported", "refused")
    expect(message).toMatch(/this version of Nessa cannot open/i)
    expect(message).not.toMatch(/(configure|config\.json|agents\.runtimes|restart)/i)
  })

  it("says the same thing about an unopenable agent whether you typed or clicked", () => {
    // A message refused for this reason falls through to the client's own
    // sentence; a control is answered here. The same failure saying two
    // different things depending on which one you did is the bug.
    expect(submissionRefusalMessage("agent-unsupported")).toBeUndefined()
    expect(controlFailureMessage("agent-unsupported", "refused")).toMatch(
      /this version of Nessa cannot open/i,
    )
  })

  it("says something different, and useful, for each refusal", () => {
    const messages = [
      imageRefusalMessage({ kind: "unsupported-file", name: "notes.pdf" }, true),
      imageRefusalMessage({ kind: "upload-failed", name: "a.png" }, true),
      imageRefusalMessage({ kind: "upload-in-flight" }, true),
      imageRefusalMessage({ kind: "too-many-images" }, true),
      imageRefusalMessage({ kind: "images-too-large" }, true),
      imageRefusalMessage({ kind: "too-many-files" }, true),
    ]
    expect(new Set(messages).size).toBe(messages.length)
    // Only the one about a file with no location turns on the surface; every
    // other refusal is the same fact wherever it is read.
    for (const refusal of [
      { kind: "upload-failed", name: "a.png" },
      { kind: "upload-in-flight" },
      { kind: "too-many-images" },
      { kind: "images-too-large" },
      { kind: "too-many-files" },
    ] as const)
      expect(imageRefusalMessage(refusal, false)).toBe(imageRefusalMessage(refusal, true))
    expect(messages[0]).toContain("notes.pdf")
    expect(messages[1]).toContain("a.png")
  })
})

describe("originals kept to paint sent turns", () => {
  /** Three tabs' worth of sent images, each `size` bytes, in the given receipts. */
  function sentImages(
    receipts: ("accepted" | "sending" | "failed" | "unknown")[],
    size: number,
  ) {
    let tabs = emptyLocalTabs()
    receipts.forEach((receipt, index) => {
      const id = `f${index}`
      tabs = attachFiles(tabs, [image(id, size)], "c0")
      tabs = changeUpload(tabs, { fileId: id, to: "uploading" })
      tabs = changeUpload(tabs, { fileId: id, to: "stored", image: reference() })
      tabs = beginSend(tabs, {
        ...submission,
        executionId: `e${index}`,
        content: tabs.conversations[0]!.draft,
      })
      tabs = {
        ...tabs,
        conversations: tabs.conversations.map((conversation) => ({
          ...conversation,
          turns: conversation.turns.map((turn) =>
            turn.from === "user" && turn.executionId === `e${index}`
              ? { ...turn, receipt }
              : turn,
          ),
        })),
      }
    })
    return tabs
  }
  const kinds = (tabs: ReturnType<typeof emptyLocalTabs>) =>
    tabs.conversations[0]!.turns.flatMap((turn) =>
      turn.from === "user" ? turn.content.map((part) => part.type) : [],
    )

  it("turns the oldest taken messages' originals into references past the budget", () => {
    const tabs = sentImages(["accepted", "accepted", "accepted"], 40)
    expect(boundSentPreviews(tabs, 120)).toBe(tabs)
    const bounded = boundSentPreviews(tabs, 100)
    expect(kinds(bounded)).toEqual(["image-reference", "file", "file"])
    // What is left in place of the original is exactly what the message named.
    expect(bounded.conversations[0]!.turns[0]).toMatchObject({
      content: [{ type: "image-reference", ...reference() }],
    })
    expect(kinds(boundSentPreviews(tabs, 0))).toEqual([
      "image-reference",
      "image-reference",
      "image-reference",
    ])
  })

  it("never takes the original from a message that may yet come back to the draft", () => {
    const tabs = sentImages(["sending", "unknown", "failed", "accepted"], 40)
    // Only the accepted one is a preview; the rest may need their bytes again.
    expect(kinds(boundSentPreviews(tabs, 0))).toEqual([
      "file",
      "file",
      "file",
      "image-reference",
    ])
  })
})

describe("the file's type decides its route, never the gesture", () => {
  const opened = (id: string, name: string, browserType: string) => ({
    ...file(id),
    name,
    mimeType: browserType,
    path: null,
  })
  const chosen = (id: string, name: string) => ({
    ...file(id),
    name,
    // What the picker hands over: a name, a path, and no idea what is in it.
    mimeType: "application/octet-stream",
    previewUrl: "",
    path: `/Users/ada/${name}`,
  })
  const attachedTo = (files: FileAttachment[]) => {
    const draft = attachFiles(emptyLocalTabs(), files, "c0").conversations[0]!.draft
    return draft.filter((part): part is FileAttachment => part.type === "file")
  }

  it("names an unopened file by its extension, so a chosen image is still an image", () => {
    // The gap this pins. A picked file arrives typed as nothing, so its name is
    // the only evidence; without the extension deciding, a `.heic` chosen with
    // + would have been pointed at by path while the same file dropped was
    // uploaded — the same file behaving differently by gesture, which is the
    // thing this rule exists to prevent.
    const [heic, png, pdf] = attachedTo([
      chosen("a", "holiday.heic"),
      chosen("b", "screenshot.png"),
      chosen("c", "report.pdf"),
    ])
    expect(heic!.mimeType).toBe("image/heic")
    expect(png!.mimeType).toBe("image/png")
    expect(isImageFile(heic!.mimeType)).toBe(true)
    expect(isImageFile(png!.mimeType)).toBe(true)
    // An image is carried, so it is never pointed at, path or no path.
    expect(linkedFile(heic!)).toBe(false)
    expect(linkedFile(png!)).toBe(false)
    // And anything else is, which is what makes it sendable at all.
    expect(pdf!.mimeType).toBe("application/octet-stream")
    expect(linkedFile(pdf!)).toBe(true)
  })

  it("gives the same file the same route whichever way it arrived", () => {
    // One file, four ways in. The browser types a drop and a paste; the picker
    // types nothing. Every one of them must land on the same route.
    const dropped = opened("a", "screenshot.png", "image/png")
    const pasted = opened("b", "screenshot.png", "image/png")
    // Some browsers report a RAW file as nothing at all, which is the same
    // evidence the picker gives.
    const untypedDrop = opened("c", "holiday.heic", "")
    const picked = chosen("d", "holiday.heic")
    const routes = attachedTo([dropped, pasted, untypedDrop, picked]).map((part) => ({
      image: isImageFile(part.mimeType),
      linked: linkedFile(part),
    }))
    expect(routes).toEqual([
      { image: true, linked: false },
      { image: true, linked: false },
      { image: true, linked: false },
      { image: true, linked: false },
    ])
  })

  it("keeps the browser's own answer ahead of the extension", () => {
    // A `.txt` the browser typed as plain text is not re-decided by its name,
    // and a name that says nothing leaves the type where it was.
    const [text, unknown] = attachedTo([
      opened("a", "notes.txt", "text/plain"),
      opened("b", "archive", ""),
    ])
    expect(text!.mimeType).toBe("text/plain")
    expect(unknown!.mimeType).toBe("application/octet-stream")
  })
})
