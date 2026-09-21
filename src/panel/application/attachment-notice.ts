import {
  declaredMediaType,
  isImageFile,
  MAX_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENTS,
  uploadFailureSummary,
  worthRetrying,
  type UploadFailure,
} from "../../conversation"

const MIB = 1024 * 1024

/** A draft file, as much of it as the notice needs: its upload state keeps its union. */
export type NoticedFile = {
  id: string
  name: string
  image: boolean
  /**
   * Whether the message can say where this file is. A file that is not an
   * image goes as a path, so this is what decides whether it can go at all.
   */
  linked: boolean
  upload:
    | { status: "not-started" | "uploading" | "stored" }
    | {
        status: "failed"
        reason: UploadFailure
      }
}

/**
 * Why the panel would not take what it was handed, as something to branch on.
 *
 * One reason per refusal that exists, because the sentence for each is
 * different and, more to the point, the advice is: a file over the per-file
 * bound is refused by every route into this composer, so it must not be sent
 * round to another one, while bytes that could not be read are exactly what
 * picking the file again can fix.
 *
 * `reading-files` and `reading-folder`: another read is still running and this
 * window takes one at a time. `sending-while-reading`: send was pressed while
 * one was. `too-many-files`, `file-too-large` and `draft-too-large`: the
 * draft's own three bounds, which used to be told as one sentence listing all
 * three. `window-budget`: every open draft together, whose figure belongs to
 * the resource store and so arrives with the refusal. `empty-folder`,
 * `folder-too-large` and `unreadable-folder`: walking a dropped folder.
 * `unreadable-files` and `unreadable-image-url`: bytes that were there and
 * could not be taken.
 */
export type AttachmentRefusal =
  | { reason: "reading-files" }
  | { reason: "reading-folder" }
  | { reason: "sending-while-reading" }
  | { reason: "too-many-files" }
  /**
   * The files over the per-file bound, in the order dropped, each with the
   * platform's own type beside its name.
   *
   * The type is carried rather than worked out from the name here, because
   * working it out is what made this notice disagree with the route it was
   * advising: a `.jp2` classified from the extension table alone is not an
   * image, so the notice offered the picker — and the picker, which asks the
   * platform, made it an image and refused it at the same bound. Two clicks,
   * forever. Whatever decides the route has to see what the route will see.
   */
  | {
      reason: "file-too-large"
      files: readonly { name: string; type: string }[]
    }
  | { reason: "draft-too-large" }
  | { reason: "window-budget"; maxMiB: number; draftsHoldFiles: boolean }
  /**
   * The file is being fetched and had not arrived before the host gave up
   * waiting. Nothing is wrong and nothing needs doing: it is still coming.
   */
  | { reason: "file-not-ready-yet"; name: string | null }
  /**
   * The file's bytes are not on this machine and nothing here can go and get
   * them — another cloud provider's placeholder, or a platform with no iCloud.
   */
  | { reason: "file-not-readable"; name: string | null }
  /**
   * This conversation's model takes no images, and an image was attached to
   * it. Said at attach rather than after an upload that could only fail.
   */
  | { reason: "images-not-supported" }
  | { reason: "empty-folder" }
  | { reason: "folder-too-large" }
  | { reason: "unreadable-folder" }
  | { reason: "unreadable-files" }
  | { reason: "unreadable-image-url" }
  /** The host's picker did not open, so nothing was chosen. */
  | { reason: "picker-unavailable" }
  /**
   * A chosen file's path is not text Nessa can pass on, so the message could
   * not say where it is. `name` is a rendering of it good enough to point at
   * the file and never good enough to be used as a path; null when even that
   * could not be had.
   */
  | { reason: "file-not-nameable"; name: string | null }
  /** A chosen file could not be read enough to describe it. */
  | { reason: "file-unreadable"; name: string | null }
  /**
   * A chosen file is somewhere the agent could not be pointed at. Refused here,
   * while the file can still be swapped, rather than at send.
   */
  | { reason: "file-not-linkable"; name: string }
  /** What was picked is not a file: a directory, a package, a pipe, a device. */
  | { reason: "file-not-a-file"; name: string | null }
  /** The filesystem did not answer about it in time. */
  | { reason: "file-unresponsive"; name: string | null }
  /**
   * The host no longer holds a read for this file. Its own reason rather than
   * one of the two above, because nothing is wrong with the file: the ticket
   * that authorised reading it is used, unknown, expired, or was evicted, and
   * picking the file again mints a new one.
   */
  | { reason: "file-must-be-chosen-again" }

/**
 * What the notification offers to do, where there is something worth doing.
 *
 * `retry-uploads` names the draft files whose upload could end differently;
 * `choose-files` opens the file picker, which is the answer only when the
 * trouble was reading the bytes this way rather than the file itself.
 */
export type AttachmentNoticeAction =
  { kind: "retry-uploads"; files: readonly string[] } | { kind: "choose-files" }

/** A short notification about the draft's files: one title, one line, one action at most. */
export type AttachmentNotice = {
  /**
   * Which of the two things the composer has to say this is.
   *
   * `draft-files` is a fact about what the draft is holding, so it lasts as
   * long as that fact and cannot be put away — the notice *is* the state.
   * `refusal` is the answer to something somebody just tried, held in the
   * panel's own memory, so it can be dismissed and it stops being shown when
   * the draft it was refused against changes.
   */
  kind: "draft-files" | "refusal"
  title: string
  description: string
  /** Null when there is nothing this panel could do about it. */
  action: AttachmentNoticeAction | null
}

/**
 * Why nothing more can be attached, said truthfully. "Remove files" is only
 * advice when there are files to remove: with every draft empty, what holds the
 * budget is messages still on their way, and those release it by arriving.
 */
export function windowBudgetMessage(maxMiB: number, draftsHoldFiles: boolean): string {
  return draftsHoldFiles
    ? `Attachments can use up to ${maxMiB} MiB across conversations. Remove files or close a conversation first.`
    : `Attachments can use up to ${maxMiB} MiB across conversations, and messages still being sent are using it. Try again once they have gone, or close a conversation.`
}

/** "photo.png is" for one, "3 files are" for several. */
function subject(files: readonly { name: string }[]): string {
  const [only] = files
  return files.length === 1 && only ? `${only.name} is` : `${files.length} files are`
}

/**
 * The notification for one refusal. Every refusal has one; none is left unsaid.
 *
 * `canChoosePaths` has no default on purpose. It used to default to `true` —
 * the desktop's answer — so a caller that had not been told simply told a
 * browser to press `+`, which is the loop this argument exists to close. An
 * omitted argument is a type error instead.
 */
export function refusalNotice(
  refusal: AttachmentRefusal,
  canChoosePaths: boolean,
): AttachmentNotice {
  const say = (
    title: string,
    description: string,
    action: AttachmentNoticeAction | null = null,
  ): AttachmentNotice => ({ kind: "refusal", title, description, action })
  switch (refusal.reason) {
    case "reading-files":
      return say(
        "Still reading files",
        "Wait for the files already here to finish, then attach the rest.",
      )
    case "reading-folder":
      return say(
        "Still reading a folder",
        "Wait for the dropped folder to finish, then attach more.",
      )
    case "sending-while-reading":
      return say("Attachments still loading", "Send again once they have finished.")
    // True whether or not the draft is holding anything: with an empty draft,
    // "remove some" would be advice about files that are not there.
    case "too-many-files":
      return say(
        "Too many files",
        `A draft holds up to ${MAX_DRAFT_ATTACHMENTS} files at a time. Attach fewer, or send what is here first.`,
      )
    // The bound is about carrying bytes, so the way out is a route that does
    // not carry them — and there is one for anything that is not an image,
    // because the picker learns where a file is and the message names that
    // instead. For an image there is no way out: it has to be uploaded, and
    // the picker holds it to this same bound, so sending somebody there would
    // be sending them to be refused again.
    case "file-too-large": {
      const overSized =
        refusal.files.length === 1 ? "File is too large" : "Files are too large"
      // Two conditions, and both have been wrong once. There has to be a
      // picker to send somebody to — a browser's file input hands back another
      // file with no location and the same refusal returns — and the file has
      // to be one the picker would route differently, judged the way the
      // picker judges it: the platform's type first, the table only after.
      const anyLinkable =
        canChoosePaths &&
        refusal.files.some(
          (file) => !isImageFile(declaredMediaType(file.name, file.type)),
        )
      return say(
        overSized,
        anyLinkable
          ? `${subject(refusal.files)} over ${MAX_ATTACHMENT_BYTES / MIB} MiB, which is as much as a drop can carry. Choose it with + instead and Nessa will tell the agent where it is, whatever it weighs.`
          : `${subject(refusal.files)} over ${MAX_ATTACHMENT_BYTES / MIB} MiB, the most one image can weigh.`,
        anyLinkable ? { kind: "choose-files" } : null,
      )
    }
    case "draft-too-large":
      return say(
        "Draft is too large",
        `One draft carries up to ${MAX_DRAFT_ATTACHMENT_BYTES / MIB} MiB of files. Remove some, or send what is here first.`,
      )
    case "window-budget":
      return say(
        "No room for more files",
        windowBudgetMessage(refusal.maxMiB, refusal.draftsHoldFiles),
      )
    // Two sentences for one symptom, because the remedies are opposite. The
    // first is "do nothing, it is coming"; the second is "this one will not
    // come on its own". Saying either of them for the other case is worse than
    // saying nothing: one sends somebody to Finder for a file that is already
    // downloading, and the other leaves them waiting for a file that is not.
    case "file-not-ready-yet":
      return say(
        "Still getting the file ready",
        `${named(refusal.name)} is being fetched from where it is stored. Attach it again in a moment.`,
        { kind: "choose-files" },
      )
    case "file-not-readable":
      return say(
        "File is not on this Mac",
        `${named(refusal.name)} is stored in the cloud and has not been downloaded. Open it once in Finder, then attach it.`,
        { kind: "choose-files" },
      )
    // The same words the draft notice uses for a draft that is already
    // holding one, because it is the same fact. What differs is only when it
    // is said: here, before an upload nobody needed, rather than after one.
    case "images-not-supported":
      return say(
        "Images not supported",
        "This agent's model doesn't take images, so it wasn't attached. Send it to an agent that does, or attach something else.",
      )
    case "empty-folder":
      return say("Folder has no files", "There is nothing in it to attach.")
    case "folder-too-large":
      return say(
        "Folder is too big to read",
        `A draft holds up to ${MAX_DRAFT_ATTACHMENTS} files, and a folder is read up to 1,000 entries in.`,
      )
    case "unreadable-folder":
      return say("Folder could not be read", "Choose the files inside it instead.", {
        kind: "choose-files",
      })
    case "unreadable-files":
      return say("Files could not be attached", "Choose them again.", {
        kind: "choose-files",
      })
    case "unreadable-image-url":
      return say(
        "Image could not be loaded",
        "Save the image first, then drop the file here.",
      )
    case "picker-unavailable":
      return say("The file picker did not open", "Try choosing files again.", {
        kind: "choose-files",
      })
    // Said rather than skipped. A selection quietly one file short is the
    // failure this whole feature is built to avoid.
    case "file-not-nameable":
      return say(
        "File could not be attached",
        `${named(refusal.name)} is named in characters Nessa cannot pass on to the agent. Rename it, then choose it again.`,
        { kind: "choose-files" },
      )
    case "file-unreadable":
      return say(
        "File could not be attached",
        `${named(refusal.name)} could not be read just now. Choose it again.`,
        { kind: "choose-files" },
      )
    case "file-not-linkable":
      return say(
        "File can't be sent from there",
        `"${refusal.name}" is in a folder Nessa cannot describe to the agent — a name that is only dots, or one with a control character in it. Move it somewhere else and choose it again.`,
        { kind: "choose-files" },
      )
    // A macOS package is the ordinary way to meet this: `.key`, `.app` and
    // `.rtfd` are directories the picker shows as single files.
    case "file-not-a-file":
      return say(
        "That is not a file",
        `${named(refusal.name)} is a folder or a package rather than a file. Choose a file inside it.`,
        { kind: "choose-files" },
      )
    case "file-unresponsive":
      return say(
        "File did not respond",
        `${named(refusal.name)} is somewhere that did not answer — a disconnected drive or a network share. Reconnect it, or choose a file somewhere else.`,
        { kind: "choose-files" },
      )
    case "file-must-be-chosen-again":
      return say(
        "Choose the file again",
        "Nessa no longer has permission to read that file. Choosing it again is all it needs.",
        { kind: "choose-files" },
      )
  }
}

/** How to refer to a file when its own name may be unusable or absent. */
function named(name: string | null): string {
  return name === null ? "One of the files chosen" : `"${name}"`
}

/**
 * What the host said when it would not hand over a chosen file, in this
 * composer's own words.
 *
 * The host answers with a typed reason, both for choosing a file and for
 * reading one, and every name it can send has a case here. `default:` is for
 * something that is not one of them — a transport fault, a command that is not
 * there — and the sentence it gives says only that the picker did not work,
 * which is all that can honestly be claimed then. It had been catching six real
 * host reasons as well, so somebody attaching a `.key` was told the picker had
 * not opened, when it demonstrably had.
 *
 * `hostRefusals` is what keeps that from happening again: it lists every name
 * the seam can carry, and a test walks it.
 */
export const hostRefusals = [
  "picker-unavailable",
  "path-not-text",
  "path-names-no-file",
  "size-unreadable",
  "not-a-regular-file",
  "filesystem-stalled",
  "file-unreadable",
  "file-too-large",
  "ticket-unavailable",
  "ticket-unknown",
  "ticket-already-used",
  "ticket-expired",
  "file-not-readable",
  "file-not-ready-yet",
  "folder-empty",
  "folder-too-large",
  "folder-unreadable",
] as const

export function pickerRefusal(error: unknown): AttachmentRefusal {
  const answer =
    error && typeof error === "object" ? (error as Record<string, unknown>) : {}
  const name = typeof answer.shown === "string" ? answer.shown : null
  switch (answer.reason) {
    case "path-not-text":
      return { reason: "file-not-nameable", name }
    case "path-names-no-file":
    case "size-unreadable":
    case "file-unreadable":
      return { reason: "file-unreadable", name }
    case "not-a-regular-file":
      return { reason: "file-not-a-file", name }
    case "filesystem-stalled":
      return { reason: "file-unresponsive", name }
    // Four host reasons, one sentence, because the panel does the same thing
    // for all four and saying otherwise would be inventing a distinction a
    // person cannot act on. They are separate on the host because *it* acts on
    // them differently; this is the boundary where that stops being true.
    case "ticket-unavailable":
    case "ticket-unknown":
    case "ticket-already-used":
    case "ticket-expired":
      return { reason: "file-must-be-chosen-again" }
    // A dropped folder, walked by the host now that the page receives no drop
    // and so cannot call `webkitGetAsEntry`. Three host reasons onto the three
    // words this panel already had for them, with the same sentences: the walk
    // moved, what it can say did not.
    // A file whose bytes are not on this machine. Two answers, because the
    // person has two different things to do: nothing at all for one, and a
    // trip to Finder for the other.
    case "file-not-ready-yet":
      return { reason: "file-not-ready-yet", name }
    case "file-not-readable":
      return { reason: "file-not-readable", name }
    case "folder-empty":
      return { reason: "empty-folder" }
    case "folder-too-large":
      return { reason: "folder-too-large" }
    case "folder-unreadable":
      return { reason: "unreadable-folder" }
    // The host will not read more than the panel would hold. Said with the
    // same words a drop of the same file gets, because it is the same bound.
    case "file-too-large":
      // The host refused to read it, so there is no platform type to carry and
      // no picker left to advise: this file already came from one.
      return {
        reason: "file-too-large",
        files: name === null ? [] : [{ name, type: "" }],
      }
    default:
      return { reason: "picker-unavailable" }
  }
}

/**
 * The drop zone's own vocabulary for a file it would not hand over. Mirrored
 * rather than imported so this layer stays clear of the design system; the call
 * site passes the zone's rejections straight in, so a reason added there stops
 * compiling here.
 */
export type DroppedFileRejection = {
  /** The platform's `type` travels with the name; see `file-too-large`. */
  file: { name: string; type: string }
  reason: "type" | "size" | "count" | "folder"
}

/**
 * One refusal for everything a drop was refused for, or nothing.
 *
 * A drop can break more than one rule at once and there is one line to say it
 * in, so the rules are ranked by how badly the wrong one reads: weight first,
 * because that is the refusal whose advice used to send somebody to a picker
 * holding the same bound, then the count, then a folder that turned out to be
 * empty.
 *
 * `type` is the zone's fourth answer and has no words here, because this panel
 * gives the zone no `accept` list and so the zone has no rule of that kind to
 * apply. Adding one is a prop, not a type change, and this would answer it with
 * nothing at all: giving the zone an `accept` means writing words here in the
 * same change. What the union does catch is a reason added upstream, which
 * stops this file compiling until it is answered.
 *
 * `folder` is reachable in the component — the zone reports a directory it was
 * told not to expand, and also one it did expand and found empty — but not from
 * here: `use-content-drop` claims any drop carrying a directory in the capture
 * phase and stops it before the zone's own drop handler runs, so folders reach
 * `use-folder-drop` instead and are refused there.
 */
export function droppedFilesRefusal(
  rejections: readonly DroppedFileRejection[],
): AttachmentRefusal | null {
  const named = (reason: DroppedFileRejection["reason"]) =>
    rejections.filter((rejection) => rejection.reason === reason)
  const tooLarge = named("size")
  if (tooLarge.length > 0)
    return {
      reason: "file-too-large",
      files: tooLarge.map((rejection) => ({
        name: rejection.file.name,
        type: rejection.file.type,
      })),
    }
  if (named("count").length > 0) return { reason: "too-many-files" }
  if (named("folder").length > 0) return { reason: "empty-folder" }
  return null
}

/**
 * What the draft's own files need said about them, or null.
 *
 * One notice for the draft, in a written-down order: an upload that failed
 * first, because it is the one carrying an action; then a file no message can
 * carry; then an agent that takes no images.
 *
 * Short on purpose. A failed upload is already marked on its tile, which also
 * carries the full reason and its own retry, so this only says that something
 * went wrong and offers the retry in one place. It is said at attach time
 * rather than saved for send: an agent that takes no images is reported as soon
 * as the gateway has said so, and a file no message can carry while it can
 * still be swapped. `imageInput` undefined means the gateway has not answered
 * yet, which is not a no. Uploads in flight or waiting are shown on their tiles.
 */
function draftFilesNotice(
  files: readonly NoticedFile[],
  imageInput: boolean | undefined,
  canChoosePaths: boolean,
): AttachmentNotice | null {
  const failed = files.flatMap((file) =>
    file.upload.status === "failed" ? [{ id: file.id, reason: file.upload.reason }] : [],
  )
  const [first] = failed
  if (first) {
    const retry = failed
      .filter((file) => worthRetrying(file.reason))
      .map((file) => file.id)
    return {
      kind: "draft-files",
      title:
        failed.length === 1
          ? "Image didn't upload"
          : `${failed.length} images didn't upload`,
      description: uploadFailureSummary(first.reason),
      action: retry.length > 0 ? { kind: "retry-uploads", files: retry } : null,
    }
  }
  // A file that is neither carried as an image nor nameable as a path cannot
  // go at all, and in the app there is no longer any way to acquire one: the
  // host owns the drag as well as the picker, so a dropped file arrives with
  // its path exactly as a picked one does. What is left is a paste, which
  // carries bytes and no location, and a browser, where nothing has a location.
  //
  // The sentence used to send everybody to `+`, which was the right advice
  // when a drop could not say where a file was and is now wrong twice over: in
  // the app the drop already did, and in a browser `+` is the page's own file
  // input and hands back another file with no path.
  if (files.some((file) => !file.image && !file.linked))
    return canChoosePaths
      ? {
          kind: "draft-files",
          title: "File can't be sent",
          description:
            "Pasted bytes have no location, and the agent needs one. Drop the file on Nessa, or choose it with +.",
          action: { kind: "choose-files" },
        }
      : {
          kind: "draft-files",
          title: "File can't be sent",
          description:
            "A browser never says where a file is, and the agent needs that. Send this one from the Nessa app.",
          action: null,
        }
  if (files.some((file) => file.image) && imageInput === false)
    return {
      kind: "draft-files",
      title: "Images not supported",
      description: "This agent doesn't take images.",
      action: null,
    }
  return null
}

/**
 * Everything the composer has to say about attachments right now, in the order
 * it is said down the screen. Empty when there is nothing to say.
 *
 * There are two things it can be saying and they are about different subjects:
 * what the draft is holding, and what the panel just turned away. The panel
 * used to render those as two competing mechanisms, each gated on the other
 * being absent, so a refusal left over from a drop suppressed a failed upload's
 * notice and took its Retry off the screen. Ranking them into one slot would
 * only reverse that arrow: a 720 MB file dropped while an upload had failed
 * would be refused with nothing on screen changing at all, and would then
 * surface later, out of context, when the upload was retried and succeeded.
 *
 * So both are said. A refusal answers something somebody did a moment ago and
 * must be visible at that moment; the draft notice is a standing fact and
 * cannot be taken down by a passing one. The refusal goes last, nearest the
 * composer, because that is where the action it answers happened. How long it
 * keeps being said is `use-file-attachments`'s: it lives until attaching,
 * removing, or sending answers it.
 *
 * Two at once is the worst case and it is a narrow one: the draft has to be
 * holding a file with something wrong with it *and* something has to have just
 * been turned away. The refusal is dismissible and stops being shown as soon as
 * the draft it was refused against changes, so the pair does not accumulate —
 * which is the answer to the "three notices over a small composer" worry, since
 * the third, the update, is itself one at most and dismissible too.
 */
export function attachmentNotices(input: {
  refusal: AttachmentRefusal | null
  files: readonly NoticedFile[]
  imageInput: boolean | undefined
  /**
   * Whether this surface has a picker that can say where a file is. False in a
   * browser, where every route hands over bytes and none says their location —
   * so there is nothing to send somebody to, and saying otherwise is a loop.
   */
  canChoosePaths: boolean
}): AttachmentNotice[] {
  const draft = draftFilesNotice(input.files, input.imageInput, input.canChoosePaths)
  return [
    ...(draft ? [draft] : []),
    ...(input.refusal ? [refusalNotice(input.refusal, input.canChoosePaths)] : []),
  ]
}
