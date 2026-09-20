import {
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
 * could not be taken. `unsupported-type`: a file outside an accepted list.
 */
export type AttachmentRefusal =
  | { reason: "reading-files" }
  | { reason: "reading-folder" }
  | { reason: "sending-while-reading" }
  | { reason: "too-many-files" }
  /** What the files over the per-file bound were called, in the order dropped. */
  | { reason: "file-too-large"; names: readonly string[] }
  | { reason: "draft-too-large" }
  | { reason: "window-budget"; maxMiB: number; draftsHoldFiles: boolean }
  | { reason: "empty-folder" }
  | { reason: "folder-too-large" }
  | { reason: "unreadable-folder" }
  | { reason: "unreadable-files" }
  | { reason: "unreadable-image-url" }
  | { reason: "unsupported-type" }

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
  title: string
  description: string
  /** Null when there is nothing this panel could do about it. */
  action: AttachmentNoticeAction | null
  /**
   * Whether somebody can put the notice away. A refusal can: it is held in this
   * panel's own memory and saying it once is the whole of its job. What the
   * draft's files are is not dismissible — the notice is the state, and it goes
   * when the state does.
   */
  dismissible: boolean
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
function subject(names: readonly string[]): string {
  const [only] = names
  return names.length === 1 && only ? `${only} is` : `${names.length} files are`
}

/** The notification for one refusal. Every refusal has one; none is left unsaid. */
export function refusalNotice(refusal: AttachmentRefusal): AttachmentNotice {
  const say = (
    title: string,
    description: string,
    action: AttachmentNoticeAction | null = null,
  ): AttachmentNotice => ({ title, description, action, dismissible: true })
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
    case "too-many-files":
      return say(
        "Too many files",
        `A draft holds up to ${MAX_DRAFT_ATTACHMENTS} files. Remove some, or send what is here first.`,
      )
    // No action, and deliberately none. The + picker holds a file to this same
    // bound, so sending somebody there is sending them to be refused again.
    case "file-too-large":
      return say(
        refusal.names.length === 1 ? "File is too large" : "Files are too large",
        `${subject(refusal.names)} over ${MAX_ATTACHMENT_BYTES / MIB} MiB, the most one attachment can weigh.`,
      )
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
    case "unsupported-type":
      return say("Wrong kind of file", "That kind of file cannot be attached here.")
  }
}

/**
 * The drop zone's own vocabulary for a file it would not hand over. Mirrored
 * rather than imported so this layer stays clear of the design system; the call
 * site passes the zone's rejections straight in, so a reason added there stops
 * compiling here.
 */
export type DroppedFileRejection = {
  file: { name: string }
  reason: "type" | "size" | "count" | "folder"
}

/**
 * One refusal for everything a drop was refused for.
 *
 * A drop can break more than one rule at once and there is one line to say it
 * in, so the rules are ranked by how badly the wrong one reads: weight first,
 * because that is the refusal whose advice used to send somebody to a picker
 * holding the same bound, then the count, then the kind, then a folder that
 * turned out to be empty.
 *
 * As the panel wires it today the zone can only answer `size` or `count`: it is
 * given no `accept` list, and a drop carrying any directory is taken by the
 * content-drop handler before the zone sees it. The other two are here because
 * the zone can produce them, not because this panel has seen one.
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
      names: tooLarge.map((rejection) => rejection.file.name),
    }
  if (named("count").length > 0) return { reason: "too-many-files" }
  if (named("type").length > 0) return { reason: "unsupported-type" }
  if (named("folder").length > 0) return { reason: "empty-folder" }
  return null
}

/**
 * The one notification over the composer about the draft's files, or null.
 *
 * One notice, one slot, one order — the panel used to render a refusal and this
 * notice as two separate things, each gated on the other being absent, so a
 * refusal left over from a drop suppressed a failed upload's notice and the
 * Retry with it. Both arrive here instead and the order is written down:
 *
 *   1. an upload that failed, because it is about a file that is in the draft
 *      right now, and because it is the notice that carries an action;
 *   2. the refusal, which is about something that never became a draft file;
 *   3. a file no message can carry, then an agent that takes no images.
 *
 * Short on purpose. A failed upload is already marked on its tile, which also
 * carries the full reason and its own retry, so this only says that something
 * went wrong and offers the retry in one place. It is said at attach time
 * rather than saved for send: an agent that takes no images is reported as soon
 * as the gateway has said so, and a file no message can carry while it can
 * still be swapped. `imageInput` undefined means the gateway has not answered
 * yet, which is not a no. Uploads in flight or waiting are shown on their tiles.
 */
export function attachmentNotice(input: {
  refusal: AttachmentRefusal | null
  files: readonly NoticedFile[]
  imageInput: boolean | undefined
}): AttachmentNotice | null {
  const failed = input.files.flatMap((file) =>
    file.upload.status === "failed" ? [{ id: file.id, reason: file.upload.reason }] : [],
  )
  const [first] = failed
  if (first) {
    const retry = failed
      .filter((file) => worthRetrying(file.reason))
      .map((file) => file.id)
    return {
      title:
        failed.length === 1
          ? "Image didn't upload"
          : `${failed.length} images didn't upload`,
      description: uploadFailureSummary(first.reason),
      action: retry.length > 0 ? { kind: "retry-uploads", files: retry } : null,
      dismissible: false,
    }
  }
  if (input.refusal) return refusalNotice(input.refusal)
  if (input.files.some((file) => !file.image))
    return {
      title: "File can't be sent",
      description: "Only images can be sent for now.",
      action: null,
      dismissible: false,
    }
  if (input.files.length > 0 && input.imageInput === false)
    return {
      title: "Images not supported",
      description: "This agent doesn't take images.",
      action: null,
      dismissible: false,
    }
  return null
}
