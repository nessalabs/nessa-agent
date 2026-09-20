/**
 * What the composer says about the draft's files, and what it offers to do.
 *
 * All of it is one pure function over two facts — the draft's files and the
 * last refusal — because the defect this replaced was two mechanisms deciding
 * separately: a refusal rendered as red text, a notification gated on that
 * refusal being absent, and so a failed upload's Retry removed from the screen
 * by a limits line left over from a drop.
 */
import { describe, expect, it, vi } from "vitest"

// The conversation barrel also exports its components, which need the whole UI
// package resolved. Its `testing` entry is the same pure functions with no
// component among them, so the barrel is mocked with that: one definition.
vi.mock("../../conversation", () => import("../../conversation/testing"))

import {
  MAX_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENTS,
  uploadFailureSummary,
} from "../../conversation/testing"
import {
  attachmentNotice,
  droppedFilesRefusal,
  refusalNotice,
  windowBudgetMessage,
  type AttachmentRefusal,
  type NoticedFile,
} from "./attachment-notice"

const stored: NoticedFile = {
  id: "a",
  name: "a.png",
  image: true,
  upload: { status: "stored" },
}

const failed = (id: string, reason: "unavailable" | "too-large"): NoticedFile => ({
  id,
  name: `${id}.heic`,
  image: true,
  upload: { status: "failed", reason },
})

/** The draft's files alone, with nothing having been turned away. */
const draft = (files: readonly NoticedFile[], imageInput: boolean | undefined = true) =>
  attachmentNotice({ refusal: null, files, imageInput })

describe("what the composer says about a draft's files", () => {
  it("says nothing when there is nothing to say", () => {
    expect(draft([], false)).toBeNull()
    expect(draft([stored])).toBeNull()
    // Not yet answered is not a no.
    expect(draft([stored], undefined)).toBeNull()
    expect(draft([{ ...stored, upload: { status: "uploading" } }])).toBeNull()
  })

  // What each reason says is the conversation's, and tested there. This is the
  // composing: short, the reason's own sentence, and a retry only for uploads
  // that trying again could change.
  it("says a failed upload briefly, and offers the retry only where it can help", () => {
    expect(draft([stored, failed("b", "unavailable")])).toEqual({
      title: "Image didn't upload",
      description: uploadFailureSummary("unavailable"),
      action: { kind: "retry-uploads", files: ["b"] },
      dismissible: false,
    })
    // The gateway's verdict on the image itself would be the same next time.
    expect(draft([failed("b", "too-large")])).toEqual({
      title: "Image didn't upload",
      description: uploadFailureSummary("too-large"),
      action: null,
      dismissible: false,
    })
    // Several failures are one notification: counted, the first one's reason,
    // and only the retryable ones behind the action.
    expect(draft([failed("b", "too-large"), failed("c", "unavailable")])).toEqual({
      title: "2 images didn't upload",
      description: uploadFailureSummary("too-large"),
      action: { kind: "retry-uploads", files: ["c"] },
      dismissible: false,
    })
    // Nothing here names a file or quotes a limit: the tile does that.
    expect(uploadFailureSummary("unavailable").length).toBeLessThan(40)
  })

  it("says briefly that a file cannot be sent, or that the agent takes no images", () => {
    expect(
      draft([
        { id: "n", name: "notes.pdf", image: false, upload: { status: "not-started" } },
      ]),
    ).toEqual({
      title: "File can't be sent",
      description: "Only images can be sent for now.",
      action: null,
      dismissible: false,
    })
    expect(draft([stored], false)).toEqual({
      title: "Images not supported",
      description: "This agent doesn't take images.",
      action: null,
      dismissible: false,
    })
  })

  it("does not let a file the draft is holding be put away", () => {
    // The notice is the state. Dismissing it would leave a failed upload with
    // nothing above the composer saying so.
    for (const notice of [
      draft([failed("b", "unavailable")]),
      draft([{ ...stored, image: false }]),
      draft([stored], false),
    ])
      expect(notice?.dismissible).toBe(false)
  })
})

describe("a refusal and a failed upload at the same time", () => {
  const refusal: AttachmentRefusal = { reason: "too-many-files" }

  it("keeps the failed upload's notice, and its retry, while a refusal is set", () => {
    // The regression. A limits line used to be rendered as its own paragraph and
    // to suppress this notification outright, taking the Retry with it.
    const notice = attachmentNotice({
      refusal,
      files: [stored, failed("b", "unavailable")],
      imageInput: true,
    })
    expect(notice).toEqual({
      title: "Image didn't upload",
      description: uploadFailureSummary("unavailable"),
      action: { kind: "retry-uploads", files: ["b"] },
      dismissible: false,
    })
    // Setting the refusal changed nothing about it at all.
    expect(notice).toEqual(draft([stored, failed("b", "unavailable")]))
  })

  it("says the refusal once nothing in the draft has failed", () => {
    expect(attachmentNotice({ refusal, files: [stored], imageInput: true })).toEqual(
      refusalNotice(refusal),
    )
    // And ahead of the quieter facts about the draft, which are not news.
    expect(
      attachmentNotice({
        refusal,
        files: [{ ...stored, image: false }],
        imageInput: false,
      }),
    ).toEqual(refusalNotice(refusal))
  })
})

describe("why something was not attached", () => {
  const bound = `${MAX_ATTACHMENT_BYTES / (1024 * 1024)} MiB`

  it("names the weight that refused the file, and offers no route with the same bound", () => {
    const one = refusalNotice({ reason: "file-too-large", names: ["holiday.mp4"] })
    expect(one.title).toBe("File is too large")
    expect(one.description).toBe(
      `holiday.mp4 is over ${bound}, the most one attachment can weigh.`,
    )
    // The whole point: the + picker applies this same bound, so it is not the
    // answer and is not offered. Nor is anything else, because there is none.
    expect(one.action).toBeNull()
    expect(one.description).not.toMatch(/\+|choos|select|pick/i)
    const several = refusalNotice({
      reason: "file-too-large",
      names: ["a.mp4", "b.mp4", "c.mp4"],
    })
    expect(several.title).toBe("Files are too large")
    expect(several.description).toBe(
      `3 files are over ${bound}, the most one attachment can weigh.`,
    )
    expect(several.action).toBeNull()
  })

  it("offers the picker for exactly the files that could not be read", () => {
    expect(refusalNotice({ reason: "unreadable-files" }).action).toEqual({
      kind: "choose-files",
    })
    expect(refusalNotice({ reason: "unreadable-folder" }).action).toEqual({
      kind: "choose-files",
    })
  })

  it("quotes the bound each of the draft's three rules is about", () => {
    expect(refusalNotice({ reason: "too-many-files" }).description).toContain(
      `up to ${MAX_DRAFT_ATTACHMENTS} files`,
    )
    expect(refusalNotice({ reason: "draft-too-large" }).description).toContain("128 MiB")
    expect(
      refusalNotice({ reason: "file-too-large", names: ["x"] }).description,
    ).toContain(bound)
  })

  it("advises removing files only when there are files to remove", () => {
    const holding = refusalNotice({
      reason: "window-budget",
      maxMiB: 256,
      draftsHoldFiles: true,
    })
    expect(holding.description).toMatch(/256 MiB.*Remove files/)
    const empty = windowBudgetMessage(256, false)
    expect(empty).toMatch(/256 MiB.*messages still being sent/)
    expect(empty).not.toMatch(/Remove files/)
  })

  it("gives every refusal a title, a line, and no invented action", () => {
    const every: AttachmentRefusal[] = [
      { reason: "reading-files" },
      { reason: "reading-folder" },
      { reason: "sending-while-reading" },
      { reason: "too-many-files" },
      { reason: "file-too-large", names: ["x.mp4"] },
      { reason: "draft-too-large" },
      { reason: "window-budget", maxMiB: 256, draftsHoldFiles: false },
      { reason: "empty-folder" },
      { reason: "folder-too-large" },
      { reason: "unreadable-folder" },
      { reason: "unreadable-files" },
      { reason: "unreadable-image-url" },
      { reason: "unsupported-type" },
    ]
    for (const refusal of every) {
      const notice = refusalNotice(refusal)
      expect(notice.title.length, refusal.reason).toBeGreaterThan(0)
      expect(notice.description.length, refusal.reason).toBeGreaterThan(0)
      // Every one of them is the panel's own memory, so every one can go away.
      expect(notice.dismissible, refusal.reason).toBe(true)
      // Retrying an upload is never what a refusal is about: nothing was
      // attached, so there is no file to retry.
      expect(notice.action?.kind, refusal.reason).not.toBe("retry-uploads")
    }
  })
})

describe("what a drop was refused for", () => {
  const rejected = (name: string, reason: "type" | "size" | "count" | "folder") => ({
    file: { name },
    reason,
  })

  it("answers a file's weight with its weight, naming the files", () => {
    expect(droppedFilesRefusal([rejected("holiday.mp4", "size")])).toEqual({
      reason: "file-too-large",
      names: ["holiday.mp4"],
    })
  })

  it("ranks weight first when one drop broke more than one rule", () => {
    // One line to say it in. Weight is the refusal whose advice used to send
    // somebody to a picker holding the same bound.
    expect(
      droppedFilesRefusal([
        rejected("twenty-first.png", "count"),
        rejected("holiday.mp4", "size"),
      ]),
    ).toEqual({ reason: "file-too-large", names: ["holiday.mp4"] })
    expect(droppedFilesRefusal([rejected("twenty-first.png", "count")])).toEqual({
      reason: "too-many-files",
    })
    expect(droppedFilesRefusal([rejected("notes.txt", "type")])).toEqual({
      reason: "unsupported-type",
    })
    expect(droppedFilesRefusal([rejected("empty", "folder")])).toEqual({
      reason: "empty-folder",
    })
  })

  it("says nothing about a drop nothing was refused from", () => {
    expect(droppedFilesRefusal([])).toBeNull()
  })
})
