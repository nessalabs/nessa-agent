/**
 * What the composer says about attachments, and what it offers to do.
 *
 * One function over two facts — the draft's files and the last refusal — and it
 * answers with a list, because they are two subjects and saying one was never a
 * reason to stop saying the other. The defect this replaced was two mechanisms
 * deciding separately: a refusal rendered as red text, a notification gated on
 * that refusal being absent, and so a failed upload's Retry taken off the
 * screen by a limits line left over from a drop. Ranking them into one slot
 * would only have reversed the arrow, so nothing here ranks.
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
  attachmentNotices,
  droppedFilesRefusal,
  hostRefusals,
  pickerRefusal,
  refusalNotice,
  windowBudgetMessage,
  type AttachmentRefusal,
  type NoticedFile,
} from "./attachment-notice"

const stored: NoticedFile = {
  id: "a",
  name: "a.png",
  image: true,
  linked: false,
  upload: { status: "stored" },
}

const failed = (id: string, reason: "unavailable" | "too-large"): NoticedFile => ({
  id,
  name: `${id}.heic`,
  image: true,
  linked: false,
  upload: { status: "failed", reason },
})

/** The draft's files alone, with nothing having been turned away. */
const draft = (
  files: readonly NoticedFile[],
  imageInput: boolean | undefined = true,
  canChoosePaths = true,
) => attachmentNotices({ refusal: null, files, imageInput, canChoosePaths })

describe("what the composer says about a draft's files", () => {
  it("says nothing when there is nothing to say", () => {
    expect(draft([], false)).toEqual([])
    expect(draft([stored])).toEqual([])
    // Not yet answered is not a no.
    expect(draft([stored], undefined)).toEqual([])
    expect(draft([{ ...stored, upload: { status: "uploading" } }])).toEqual([])
  })

  // What each reason says is the conversation's, and tested there. This is the
  // composing: short, the reason's own sentence, and a retry only for uploads
  // that trying again could change.
  it("says a failed upload briefly, and offers the retry only where it can help", () => {
    expect(draft([stored, failed("b", "unavailable")])).toEqual([
      {
        kind: "draft-files",
        title: "Image didn't upload",
        description: uploadFailureSummary("unavailable"),
        action: { kind: "retry-uploads", files: ["b"] },
      },
    ])
    // The gateway's verdict on the image itself would be the same next time.
    expect(draft([failed("b", "too-large")])).toEqual([
      {
        kind: "draft-files",
        title: "Image didn't upload",
        description: uploadFailureSummary("too-large"),
        action: null,
      },
    ])
    // Several failures are one notification: counted, the first one's reason,
    // and only the retryable ones behind the action.
    expect(draft([failed("b", "too-large"), failed("c", "unavailable")])).toEqual([
      {
        kind: "draft-files",
        title: "2 images didn't upload",
        description: uploadFailureSummary("too-large"),
        action: { kind: "retry-uploads", files: ["c"] },
      },
    ])
    // Nothing here names a file or quotes a limit: the tile does that.
    expect(uploadFailureSummary("unavailable").length).toBeLessThan(40)
  })

  it("says briefly that a file cannot be sent, or that the agent takes no images", () => {
    // Not an image and nothing said where it is: a drop or a paste, which hand
    // over bytes and never their location. In the app there is a picker that
    // does, so it is offered.
    expect(
      draft([
        {
          id: "n",
          name: "notes.pdf",
          image: false,
          linked: false,
          upload: { status: "not-started" },
        },
      ]),
    ).toEqual([
      {
        kind: "draft-files",
        title: "File can't be sent",
        description:
          "Pasted bytes have no location, and the agent needs one. Drop the file on Nessa, or choose it with +.",
        // Offered, because in the Nessa app it genuinely is the way out: the
        // same file chosen rather than dropped is sendable.
        action: { kind: "choose-files" },
      },
    ])
    // In a browser there is no picker to send anybody to — its file input
    // hands back another file with no path — so the offer would be a loop, and
    // the sentence names the app instead of blaming the file.
    const inBrowser = draft(
      [
        {
          id: "n",
          name: "notes.pdf",
          image: false,
          linked: false,
          upload: { status: "not-started" },
        },
      ],
      true,
      false,
    )
    expect(inBrowser[0]?.action).toBeNull()
    expect(inBrowser[0]?.description).toContain("Nessa app")
    expect(inBrowser[0]?.description).not.toMatch(/\+/)
    // The same file chosen through the host's picker says nothing at all: it
    // goes as a path, and there is no upload to wait for or report.
    expect(
      draft([
        {
          id: "n",
          name: "notes.pdf",
          image: false,
          linked: true,
          upload: { status: "not-started" },
        },
      ]),
    ).toEqual([])
    // And an agent that takes no images has nothing to say about it either.
    expect(
      draft(
        [
          {
            id: "n",
            name: "notes.pdf",
            image: false,
            linked: true,
            upload: { status: "not-started" },
          },
        ],
        false,
      ),
    ).toEqual([])
    expect(draft([stored], false)).toEqual([
      {
        kind: "draft-files",
        title: "Images not supported",
        description: "This agent doesn't take images.",
        action: null,
      },
    ])
  })

  it("never says more than one thing about the draft at a time", () => {
    // A draft can break several of these at once; there is one notice for it.
    const notices = draft(
      [failed("b", "unavailable"), { ...stored, image: false }],
      false,
    )
    expect(notices).toHaveLength(1)
    expect(notices[0]?.title).toBe("Image didn't upload")
  })
})

describe("a refusal and the draft's files at the same time", () => {
  const refusal: AttachmentRefusal = {
    reason: "file-too-large",
    files: [{ name: "holiday.mp4", type: "video/mp4" }],
  }

  it("says both, and neither can take the other off the screen", () => {
    // The regression, both ways round. A limits line used to suppress the
    // failed upload's notification outright, taking its Retry with it; ranking
    // the other way would have thrown the refusal away at the moment somebody
    // dropped the file.
    const notices = attachmentNotices({
      refusal,
      files: [stored, failed("b", "unavailable")],
      imageInput: true,
      canChoosePaths: true,
    })
    expect(notices.map((notice) => notice.kind)).toEqual(["draft-files", "refusal"])
    expect(notices[0]).toEqual({
      kind: "draft-files",
      title: "Image didn't upload",
      description: uploadFailureSummary("unavailable"),
      action: { kind: "retry-uploads", files: ["b"] },
    })
    expect(notices[1]).toEqual(refusalNotice(refusal, true))
    // The draft's notice is word for word the one it would be with no refusal
    // set at all, action included.
    expect(notices[0]).toEqual(draft([stored, failed("b", "unavailable")])[0])
  })

  it("says the refusal on its own when the draft has nothing wrong with it", () => {
    expect(
      attachmentNotices({
        refusal,
        files: [stored],
        imageInput: true,
        canChoosePaths: true,
      }),
    ).toEqual([refusalNotice(refusal, true)])
  })

  it("puts the refusal last, nearest the composer, wherever it appears", () => {
    for (const files of [
      [stored],
      [failed("b", "unavailable")],
      [{ ...stored, image: false }],
    ])
      expect(
        attachmentNotices({ refusal, files, imageInput: true, canChoosePaths: true }).at(
          -1,
        ),
      ).toEqual(refusalNotice(refusal, true))
  })
})

describe("why something was not attached", () => {
  const bound = `${MAX_ATTACHMENT_BYTES / (1024 * 1024)} MiB`

  it("sends an oversized file to the route that does not carry its bytes", () => {
    // The headline of the report this feature came from: a 720 MB video,
    // dropped, refused. The bound is about carrying bytes across the drop, and
    // a video does not have to be carried — the picker learns where it is and
    // the message names that. So the way out is named, and offered.
    const one = refusalNotice(
      {
        reason: "file-too-large",
        files: [{ name: "holiday.mp4", type: "video/mp4" }],
      },
      true,
    )
    expect(one.title).toBe("File is too large")
    expect(one.description).toContain(`holiday.mp4 is over ${bound}`)
    expect(one.description).toMatch(/choose it with \+/i)
    expect(one.action).toEqual({ kind: "choose-files" })
    const several = refusalNotice(
      {
        reason: "file-too-large",
        files: [
          { name: "a.mp4", type: "video/mp4" },
          { name: "b.mp4", type: "video/mp4" },
          { name: "c.mp4", type: "video/mp4" },
        ],
      },
      true,
    )
    expect(several.title).toBe("Files are too large")
    expect(several.description).toContain(`3 files are over ${bound}`)
    expect(several.action).toEqual({ kind: "choose-files" })
  })

  it("offers nothing in a browser, where the picker it would name is a loop", () => {
    // A browser's file input hands back another file with no location, so the
    // same refusal comes straight back. Offering it would be advice that
    // cannot work — the defect this composer already learned once, with the
    // oversized drop sent to a picker holding the same bound.
    const dropped: AttachmentRefusal = {
      reason: "file-too-large",
      files: [{ name: "holiday.mp4", type: "video/mp4" }],
    }
    expect(refusalNotice(dropped, true).action).toEqual({ kind: "choose-files" })
    const inBrowser = refusalNotice(dropped, false)
    expect(inBrowser.action).toBeNull()
    expect(inBrowser.description).not.toMatch(/choos|select|pick/i)
    // Re-reading bytes is still worth offering: a browser's input can do that.
    expect(refusalNotice({ reason: "unreadable-files" }, false).action).toEqual({
      kind: "choose-files",
    })
  })

  it("judges the route the way the picker will, not the way a name reads", () => {
    // The two-click loop. This notice used to classify from the name alone,
    // so a `.jp2` — an image the platform knows and this app's extension table
    // does not — looked like something the picker would carry by path. It
    // offered the picker; the picker asked the platform, got `image/jp2`,
    // routed it as an image, and refused it at the same bound with the same
    // notice and the same button. Forever.
    const scan = refusalNotice(
      {
        reason: "file-too-large",
        files: [{ name: "scan.jp2", type: "image/jp2" }],
      },
      true,
    )
    expect(scan.action).toBeNull()
    expect(scan.description).not.toMatch(/choos|select|pick/i)

    // The same file with no platform type behind it is still an image to the
    // table for the formats it does know, and still not offered.
    const raw = refusalNotice(
      {
        reason: "file-too-large",
        files: [{ name: "holiday.cr3", type: "" }],
      },
      true,
    )
    expect(raw.action).toBeNull()

    // And a genuine non-image is offered, which is the case the advice exists
    // for — platform-typed or not.
    for (const type of ["video/mp4", ""])
      expect(
        refusalNotice(
          {
            reason: "file-too-large",
            files: [{ name: "holiday.mp4", type }],
          },
          true,
        ).action,
      ).toEqual({ kind: "choose-files" })
  })

  it("offers no route for an oversized image, because every route weighs it", () => {
    // An image has to be uploaded wherever it came from, and the picker holds
    // it to this same bound, so sending somebody there is sending them to be
    // refused again. Naming the route would be worse than saying nothing.
    const image = refusalNotice(
      {
        reason: "file-too-large",
        files: [{ name: "holiday.heic", type: "image/heic" }],
      },
      true,
    )
    expect(image.description).toBe(
      `holiday.heic is over ${bound}, the most one image can weigh.`,
    )
    expect(image.action).toBeNull()
    expect(image.description).not.toMatch(/\+|choos|select|pick/i)
    // A mixed drop keeps the offer, because it helps the files it applies to.
    const mixed = refusalNotice(
      {
        reason: "file-too-large",
        files: [
          { name: "holiday.heic", type: "image/heic" },
          { name: "holiday.mp4", type: "video/mp4" },
        ],
      },
      true,
    )
    expect(mixed.action).toEqual({ kind: "choose-files" })
  })

  it("offers the picker for exactly the files that could not be read", () => {
    expect(refusalNotice({ reason: "unreadable-files" }, true).action).toEqual({
      kind: "choose-files",
    })
    expect(refusalNotice({ reason: "unreadable-folder" }, true).action).toEqual({
      kind: "choose-files",
    })
  })

  it("answers every reason that can cross the seam, and never with the wrong one", () => {
    // `default:` used to swallow six of the twelve, so attaching a `.key` — a
    // macOS package, which is a directory the picker shows as a file — told
    // somebody the picker had not opened, when it demonstrably had.
    const pickerDidNotOpen = refusalNotice({ reason: "picker-unavailable" }, true)
    // A folder with nothing in it, and one too big to read, are the two the
    // person cannot answer by choosing again: there is nothing inside the one
    // and too much inside the other. Every other reason offers the picker.
    const nothingToPress = ["folder-empty", "folder-too-large"]
    for (const reason of hostRefusals) {
      const refusal = pickerRefusal({ reason, shown: "thing.key", detail: null })
      const notice = refusalNotice(refusal, true)
      // Every one has words of its own.
      expect(notice.description.length).toBeGreaterThan(0)
      expect(notice.action, reason).toEqual(
        nothingToPress.includes(reason) ? null : { kind: "choose-files" },
      )
      if (reason === "picker-unavailable") continue
      // And none of them is told as the one that means something else.
      expect([reason, notice.title]).not.toEqual([reason, pickerDidNotOpen.title])
    }

    // The four ticket reasons deliberately share one sentence: the panel does
    // the same thing for all four, and inventing a distinction a person cannot
    // act on would be worse than admitting there is none.
    const ticketed = [
      "ticket-unavailable",
      "ticket-unknown",
      "ticket-already-used",
      "ticket-expired",
    ] as const
    const answers = ticketed.map((reason) =>
      pickerRefusal({ reason, shown: null, detail: null }),
    )
    expect(new Set(answers.map((answer) => answer.reason)).size).toBe(1)
    // And none of them names a file: the host withholds the path on purpose.
    for (const answer of answers) expect(JSON.stringify(answer)).not.toContain("thing")

    // Something that is not one of the host's names still lands on the picker
    // sentence, which is the only honest thing to say about it.
    for (const other of [new Error("no such command"), "boom", null, {}, { reason: "x" }])
      expect(pickerRefusal(other)).toEqual({ reason: "picker-unavailable" })
  })

  it("says why the host would not hand a chosen file over, in this composer's words", () => {
    // The reasons the host can answer with, each turned into one sentence a
    // person can act on. A file it could not name is said, never skipped.
    expect(pickerRefusal({ reason: "path-not-text", shown: "rep?rt.pdf" })).toEqual({
      reason: "file-not-nameable",
      name: "rep?rt.pdf",
    })
    expect(pickerRefusal({ reason: "size-unreadable", shown: "gone.pdf" })).toEqual({
      reason: "file-unreadable",
      name: "gone.pdf",
    })
    expect(pickerRefusal({ reason: "path-names-no-file", shown: null })).toEqual({
      reason: "file-unreadable",
      name: null,
    })
    expect(pickerRefusal({ reason: "picker-unavailable" })).toEqual({
      reason: "picker-unavailable",
    })

    // Anything that is not one of the host's typed answers — a transport
    // fault, a command that is not there, a string — is the picker not
    // working, which is all that can honestly be claimed.
    for (const other of [new Error("no such command"), "boom", null, undefined, {}])
      expect(pickerRefusal(other)).toEqual({ reason: "picker-unavailable" })

    // A file whose name could not be had at all still gets a sentence.
    expect(
      refusalNotice({ reason: "file-not-nameable", name: null }, true).description,
    ).toContain("One of the files chosen")
    expect(
      refusalNotice({ reason: "file-not-nameable", name: "rep?rt.pdf" }, true)
        .description,
    ).toContain(`"rep?rt.pdf"`)
    // Each one offers the picker: choosing again is what could end differently.
    for (const refusal of [
      { reason: "picker-unavailable" },
      { reason: "file-not-nameable", name: null },
      { reason: "file-unreadable", name: "a.pdf" },
    ] as const)
      expect(refusalNotice(refusal, true).action).toEqual({ kind: "choose-files" })
  })

  it("quotes the bound each of the draft's three rules is about", () => {
    expect(refusalNotice({ reason: "too-many-files" }, true).description).toContain(
      `up to ${MAX_DRAFT_ATTACHMENTS} files`,
    )
    expect(refusalNotice({ reason: "draft-too-large" }, true).description).toContain(
      "128 MiB",
    )
    expect(
      refusalNotice({ reason: "file-too-large", files: [{ name: "x", type: "" }] }, true)
        .description,
    ).toContain(bound)
  })

  it("does not advise removing files that may not be there", () => {
    // Twenty-one files dropped into an empty draft breaks the count rule, and
    // "remove some" would then be about nothing.
    expect(refusalNotice({ reason: "too-many-files" }, true).description).not.toMatch(
      /remove/i,
    )
    const holding = refusalNotice(
      {
        reason: "window-budget",
        maxMiB: 256,
        draftsHoldFiles: true,
      },
      true,
    )
    expect(holding.description).toMatch(/256 MiB.*Remove files/)
    const empty = windowBudgetMessage(256, false)
    expect(empty).toMatch(/256 MiB.*messages still being sent/)
    expect(empty).not.toMatch(/Remove files/)
  })

  it("gives every refusal a title, a line, and no invented action", () => {
    /**
     * One sample per reason, and the type says so.
     *
     * A hand-written array was a list that had to be remembered, and it was
     * not: it had drifted to twelve of twenty-two, so `images-not-supported`,
     * `picker-unavailable`, every `file-*` refusal and the `conversation-closed`
     * added last round were all outside it. That is precisely the "closed by
     * listing the cases" pattern ADR 0013 says this feature is done with.
     *
     * A total `Record` keyed on the union's own discriminant is not a list: a
     * reason added to `AttachmentRefusal` without a sample here is a type
     * error, and a sample whose `reason` does not match its key is one too.
     */
    const every: {
      [K in AttachmentRefusal["reason"]]: Extract<AttachmentRefusal, { reason: K }>
    } = {
      "reading-files": { reason: "reading-files" },
      "reading-folder": { reason: "reading-folder" },
      "sending-while-reading": { reason: "sending-while-reading" },
      "too-many-files": { reason: "too-many-files" },
      "file-too-large": {
        reason: "file-too-large",
        files: [{ name: "x.mp4", type: "video/mp4" }],
      },
      "draft-too-large": { reason: "draft-too-large" },
      "window-budget": { reason: "window-budget", maxMiB: 256, draftsHoldFiles: false },
      "file-not-ready-yet": { reason: "file-not-ready-yet", name: "amica.pdf" },
      "file-not-readable": { reason: "file-not-readable", name: "amica.pdf" },
      "images-not-supported": { reason: "images-not-supported" },
      "conversation-closed": { reason: "conversation-closed" },
      "empty-folder": { reason: "empty-folder" },
      "folder-too-large": { reason: "folder-too-large" },
      "unreadable-folder": { reason: "unreadable-folder" },
      "unreadable-files": { reason: "unreadable-files" },
      "unreadable-image-url": { reason: "unreadable-image-url" },
      "picker-unavailable": { reason: "picker-unavailable" },
      "file-not-nameable": { reason: "file-not-nameable", name: "rep?rt.pdf" },
      "file-unreadable": { reason: "file-unreadable", name: "a.png" },
      "file-not-linkable": { reason: "file-not-linkable", name: "report.pdf" },
      "file-not-a-file": { reason: "file-not-a-file", name: "pipe" },
      "file-unresponsive": { reason: "file-unresponsive", name: "on-a-mount.pdf" },
      "file-must-be-chosen-again": { reason: "file-must-be-chosen-again" },
    }
    for (const refusal of Object.values(every) as AttachmentRefusal[]) {
      const notice = refusalNotice(refusal, true)
      expect(notice.kind, refusal.reason).toBe("refusal")
      expect(notice.title.length, refusal.reason).toBeGreaterThan(0)
      expect(notice.description.length, refusal.reason).toBeGreaterThan(0)
      // Retrying an upload is never what a refusal is about: nothing was
      // attached, so there is no file to retry.
      expect(notice.action?.kind, refusal.reason).not.toBe("retry-uploads")
    }
  })
})

describe("what a drop was refused for", () => {
  const rejected = (
    name: string,
    reason: "type" | "size" | "count" | "folder",
    type = "",
  ) => ({ file: { name, type }, reason })

  it("answers a file's weight with its weight, naming the files", () => {
    expect(droppedFilesRefusal([rejected("holiday.mp4", "size", "video/mp4")])).toEqual({
      reason: "file-too-large",
      files: [{ name: "holiday.mp4", type: "video/mp4" }],
    })
  })

  it("ranks weight first when one drop broke more than one rule", () => {
    // One line to say it in. Weight is the refusal whose advice used to send
    // somebody to a picker holding the same bound.
    expect(
      droppedFilesRefusal([
        rejected("twenty-first.png", "count"),
        rejected("holiday.mp4", "size", "video/mp4"),
      ]),
    ).toEqual({
      reason: "file-too-large",
      files: [{ name: "holiday.mp4", type: "video/mp4" }],
    })
    expect(droppedFilesRefusal([rejected("twenty-first.png", "count")])).toEqual({
      reason: "too-many-files",
    })
    expect(droppedFilesRefusal([rejected("empty", "folder")])).toEqual({
      reason: "empty-folder",
    })
  })

  it("says nothing about a drop nothing was refused from, or refused only by kind", () => {
    expect(droppedFilesRefusal([])).toBeNull()
    // This panel gives the zone no `accept` list, so it has no kind rule to
    // apply and nothing can arrive here as `type`. It stays in the parameter so
    // that adding one is a compile error away from being noticed — and when
    // that happens it needs words, not this.
    expect(droppedFilesRefusal([rejected("notes.txt", "type")])).toBeNull()
  })
})
