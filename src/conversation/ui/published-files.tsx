import { MAX_IMAGE_ATTACHMENT_BYTES } from "@nessa/client"
import { Download, File as FileIcon, Image as ImageIcon } from "lucide-react"
import { Button } from "@nessa-ui/react/button"
import { Card } from "@nessa-ui/react/card"
import { formatFileSize } from "@nessa-ui/react/file-preview"
import { previewableImage, type PublishedFile } from "../model"
import {
  HELD_FILE_NOT_SAVED_TEXT,
  heldFileFailureText,
  useHeldFile,
  useRetryHeldFile,
  useSaveHeldFile,
  type HeldFileSaving,
} from "./held-files"

/**
 * Whether a published file is shown as a picture in its card: a type this
 * window can paint (`previewableImage`), and no larger than an image the
 * gateway takes into a conversation. A bigger one is still saved whole; it is
 * not read just to be looked at.
 */
export function previewedInCard(file: PublishedFile): boolean {
  return previewableImage(file.mimeType) && file.size <= MAX_IMAGE_ATTACHMENT_BYTES
}

/**
 * The files an agent published, where they were published: one card each, by
 * name and size, with a way to save it. A picture this window can paint
 * (`previewedInCard`) is shown in the card, read back from the conversation
 * and verified first; anything else is shown by name only, and read back only
 * when somebody saves it — through the host's own way of saving, never a link
 * in the page. A file the conversation no longer keeps says so, and its record
 * stays.
 *
 * Renders nothing for none, so a turn that published nothing is unchanged.
 */
export function PublishedFiles({
  conversationId,
  files,
}: {
  /** The gateway's identity for the conversation that holds them. */
  conversationId: string | undefined
  files: readonly PublishedFile[]
}) {
  if (files.length === 0) return null
  return (
    <ul
      aria-label={
        files.length === 1 ? "1 published file" : `${files.length} published files`
      }
      className="nessa-published-files m-0 flex list-none flex-col items-start gap-2 p-0"
      data-published-files
    >
      {files.map((file, index) => (
        // The same file may be published twice; its place tells the two apart.
        <li key={`${file.digest}:${file.mimeType}:${index}`} className="max-w-full">
          <PublishedFileCard conversationId={conversationId} file={file} />
        </li>
      ))}
    </ul>
  )
}

function PublishedFileCard({
  conversationId,
  file,
}: {
  conversationId: string | undefined
  file: PublishedFile
}) {
  const previewed = previewedInCard(file)
  const size = formatFileSize(file.size)
  return (
    <Card
      role="group"
      aria-label={`${file.name}, ${size}`}
      className="w-80 max-w-full gap-0 overflow-hidden py-0 shadow-none"
      data-published-file={file.name}
      title={`${file.name}, ${size}`}
    >
      {previewed ? (
        <PublishedPicture conversationId={conversationId} file={file} />
      ) : null}
      <div className="flex min-w-0 items-center gap-2 py-1.5 ps-3 pe-1.5">
        {previewableImage(file.mimeType) ? (
          <ImageIcon
            aria-hidden="true"
            className="size-4 shrink-0 text-muted-foreground"
          />
        ) : (
          <FileIcon
            aria-hidden="true"
            className="size-4 shrink-0 text-muted-foreground"
          />
        )}
        <span className="min-w-0 flex-1 truncate font-sans nessa-text-2">
          {file.name}
        </span>
        <span className="shrink-0 font-sans nessa-text-1 text-muted-foreground">
          {size}
        </span>
        <SaveButton conversationId={conversationId} file={file} />
      </div>
    </Card>
  )
}

const pictureArea = "flex h-48 w-full items-center justify-center bg-accent"

/** The picture itself, once verified; until then, and if it never is, a labelled stand-in. */
function PublishedPicture({
  conversationId,
  file,
}: {
  conversationId: string | undefined
  file: PublishedFile
}) {
  const shown = useHeldFile(conversationId, file)
  const retry = useRetryHeldFile(conversationId, file)
  if (shown?.state === "shown")
    return (
      <span className={pictureArea} data-held-file="shown">
        <img
          src={shown.url}
          alt={file.name}
          draggable={false}
          className="max-h-full max-w-full object-contain"
        />
      </span>
    )
  if (shown?.state === "failed")
    return (
      <span
        className={`${pictureArea} flex-col gap-1 px-3 text-center`}
        data-held-file="failed"
      >
        <span className="font-sans nessa-text-1 text-muted-foreground">
          {heldFileFailureText(shown.reason)}
        </span>
        {shown.reason === "unavailable" && retry ? (
          <Button type="button" variant="plain" size="sm" onClick={retry}>
            Retry
          </Button>
        ) : null}
      </span>
    )
  return (
    <span
      className={`${pictureArea} animate-pulse`}
      aria-busy={shown ? true : undefined}
      data-held-file={shown?.state ?? "unread"}
    />
  )
}

function SaveButton({
  conversationId,
  file,
}: {
  conversationId: string | undefined
  file: PublishedFile
}) {
  const { saving, save } = useSaveHeldFile(conversationId, file, file.name)
  if (!save) return null
  return (
    <>
      <SavingNote saving={saving} />
      <Button
        type="button"
        variant="plain"
        size="icon-sm"
        aria-label={`Save ${file.name}`}
        title={`Save ${file.name}`}
        disabled={saving.state === "saving"}
        aria-busy={saving.state === "saving" || undefined}
        onClick={save}
      >
        <Download aria-hidden="true" />
      </Button>
    </>
  )
}

/**
 * Says when a save failed. The live region stays mounted, empty until then,
 * so a screen reader hears the words when they appear; empty, it gives back
 * the gap beside it.
 */
function SavingNote({ saving }: { saving: HeldFileSaving }) {
  return (
    <span
      role="status"
      className="shrink-0 font-sans nessa-text-1 text-destructive empty:-ms-2"
    >
      {saving.state === "failed" ? HELD_FILE_NOT_SAVED_TEXT : null}
    </span>
  )
}
