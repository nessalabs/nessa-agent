/**
 * A file a conversation holds — an image uploaded into it, or a file its agent
 * published — named exactly as the conversation's view names it. The same
 * bytes held as another media type are another file. The view never carries
 * the bytes; they are read back by this name, and handed over only once they
 * hash to `digest`.
 */
export type HeldFile = {
  /** `sha256:` and 64 lowercase hexadecimal digits. */
  digest: string
  /** Lowercase media type without parameters. */
  mimeType: string
  /** Exact length in bytes. */
  size: number
}

/**
 * A file the conversation's agent published, as its view lists it: the turn it
 * was published in, when one was running, and the name it was published
 * under — for people only; it names no place on any machine. The record
 * outlives the bytes: whether they are still held is learnt by reading them.
 */
export type PublishedFile = HeldFile & {
  name: string
  /** The turn that was running when it was published; absent when none was. */
  executionId?: string
}

/**
 * Why a held file could not be shown or saved, in the panel's own words.
 *
 * - `not-held`: the conversation keeps it no longer — expired, or let go of
 *   when it closed. The record stays; the bytes are gone for good.
 * - `deleted`: somebody deleted the conversation.
 * - `damaged`: what arrived was not the file its name says, so none of it is
 *   shown or saved.
 * - `unavailable`: anything else — no gateway, a busy one, one that could not
 *   read it now, or an answer this window does not understand. Trying again
 *   later may work.
 */
export type HeldFileFailure = "not-held" | "deleted" | "damaged" | "unavailable"

/** Where each published file is shown in a transcript (`placePublished`). */
export type PublishedPlacement<File extends PublishedFile = PublishedFile> = {
  /** Under the turn that published them, by its execution id, in the order recorded. */
  byExecution: Map<string, File[]>
  /** Published by a turn the bounded view left out: older than every turn shown. */
  before: File[]
  /** Published while no turn ran: after the last turn. */
  after: File[]
}

/**
 * Where the files a conversation's agent published go among the turns shown.
 *
 * A file goes under the turn that was running when it was published. One whose
 * turn is not shown was published by a turn the bounded view left out — every
 * turn the view leaves out is older than the ones it keeps — so it goes before
 * the first turn shown. One published while no turn ran has no turn to belong
 * to, and goes after the last. Nothing published is dropped from view, and the
 * order the view recorded them in is kept within each place.
 *
 * The one rule for every transcript that shows them, so the panel and the
 * desktop window cannot place the same file in two different places.
 */
export function placePublished<File extends PublishedFile>(
  files: readonly File[],
  shownExecutionIds: Iterable<string>,
): PublishedPlacement<File> {
  const shown = new Set(shownExecutionIds)
  const placement: PublishedPlacement<File> = {
    byExecution: new Map(),
    before: [],
    after: [],
  }
  for (const file of files) {
    if (file.executionId === undefined) placement.after.push(file)
    else if (!shown.has(file.executionId)) placement.before.push(file)
    else
      placement.byExecution.set(file.executionId, [
        ...(placement.byExecution.get(file.executionId) ?? []),
        file,
      ])
  }
  return placement
}
