import type * as React from "react"
import { File as FileIcon, Image as ImageIcon, RotateCw } from "lucide-react"
import { Button } from "@nessa-ui/react/button"
import {
  imageReferenceLabel,
  isImageFile,
  linkedFile,
  previewableImage,
  type ImageReference,
  type MessageContent,
} from "../model"
import { heldFileFailureText, useHeldFile, useRetryHeldFile } from "./held-files"

// The composer's attachment tile, as a shape: square, rounded, on the accent wash.
const tile = "flex h-16 items-center justify-center overflow-hidden rounded-xl bg-accent"

/**
 * The images a sent turn carried, as tiles under its text.
 *
 * A turn sent from this window still has its object URL, so its tile is the
 * picture as it was attached — the original, not the copy the gateway stored.
 * A turn read back from the gateway — after a reload, or sent from another
 * surface — is known only by reference: its tile is the stored copy, read back
 * from `conversationId` by that reference and painted only once its size and
 * digest are verified (`useHeldFile`). Until then, and for good if there is
 * nothing to read it with, the tile is a labelled placeholder saying what it
 * was and how big; one that could not be read says why instead.
 *
 * A file the turn pointed the agent at gets a tile too, labelled with its name.
 * Nobody ever held its bytes — the message carried where it is, not what is in
 * it — so there is nothing to paint and nothing to fetch, and its full path is
 * the tile's title rather than its label, which would not fit and is not what
 * the person picked it by.
 *
 * Renders nothing for a turn with no attachments, so a text turn's markup is
 * unchanged. An attachment-only turn renders only this, which is why every tile
 * has a name a screen reader and a pointer can both reach, not only a picture.
 */
export function MessageImages({
  content,
  conversationId,
}: {
  content: Readonly<MessageContent>
  /** The gateway's identity for the conversation, to read referenced images from. */
  conversationId?: string
}) {
  type Tile = {
    key: string
    label: string
    title: string
    src: string | undefined
    file?: true
    /** Held by the conversation: painted from it once read back. */
    held?: ImageReference
  }
  const tiles = content.flatMap((part, index): Tile[] => {
    if (part.type === "file" && isImageFile(part.resource.mimeType))
      return [
        {
          key: part.placementId,
          label: part.resource.name,
          title: part.resource.name,
          // An original the webview cannot paint (HEIC, RAW) gets the labelled
          // tile too, by its file name, rather than a broken picture.
          src: previewableImage(part.resource.mimeType)
            ? part.resource.previewUrl
            : undefined,
        },
      ]
    if (part.type === "file" && linkedFile(part.resource))
      return [
        {
          key: part.placementId,
          label: part.resource.name,
          title: part.resource.path ?? part.resource.name,
          src: undefined,
          file: true,
        },
      ]
    if (part.type === "image-reference")
      return [
        {
          // The same image may be sent twice in one message; position disambiguates.
          key: `${part.digest}:${index}`,
          label: imageReferenceLabel(part.mimeType, part.size),
          title: imageReferenceLabel(part.mimeType, part.size),
          src: undefined,
          held: { digest: part.digest, mimeType: part.mimeType, size: part.size },
        },
      ]
    if (part.type === "uploaded-file-reference")
      return [
        {
          key: `${part.digest}:${index}`,
          label: part.name,
          title: part.name,
          src: undefined,
          file: true,
        },
      ]
    // Read back from the gateway: the path is all there ever was.
    if (part.type === "file-reference")
      return [
        {
          key: `${part.path}:${index}`,
          label: part.path.slice(part.path.lastIndexOf("/") + 1),
          title: part.path,
          src: undefined,
          file: true,
        },
      ]
    return []
  })
  if (tiles.length === 0) return null
  return (
    <ul
      aria-label={tiles.length === 1 ? "1 attachment" : `${tiles.length} attachments`}
      className="nessa-message-images m-0 flex list-none flex-wrap gap-1.5 p-0 [&:not(:first-child)]:mt-2"
    >
      {tiles.map((item) => (
        <li key={item.key} title={item.title} className="inline-flex">
          {item.held ? (
            <HeldImageTile
              conversationId={conversationId}
              image={item.held}
              label={item.label}
            />
          ) : item.src ? (
            <Picture src={item.src} label={item.label} />
          ) : (
            <Placeholder label={item.label} file={item.file} />
          )}
        </li>
      ))}
    </ul>
  )
}

/**
 * An image the conversation holds: its verified copy, or the placeholder until
 * there is one. One that could not be read says why, in the words its card
 * would use; one that could not be read now can be asked for again.
 */
function HeldImageTile({
  conversationId,
  image,
  label,
}: {
  conversationId: string | undefined
  image: ImageReference
  label: string
}) {
  const shown = useHeldFile(conversationId, image)
  const retry = useRetryHeldFile(conversationId, image)
  if (shown?.state === "shown") return <Picture src={shown.url} label={label} />
  if (shown?.state !== "failed") return <Placeholder label={label} />
  const reason = heldFileFailureText(shown.reason)
  return (
    <Placeholder label={reason} failed>
      {shown.reason === "unavailable" && retry ? (
        <Button
          type="button"
          variant="plain"
          size="22"
          shape="pill"
          aria-label={`${label}: ${reason}. Retry`}
          onClick={retry}
        >
          <RotateCw aria-hidden="true" />
          Retry
        </Button>
      ) : null}
    </Placeholder>
  )
}

function Picture({ src, label }: { src: string; label: string }) {
  return (
    <span className={`${tile} w-16`}>
      <img src={src} alt={label} className="size-full object-cover" />
    </span>
  )
}

function Placeholder({
  label,
  file,
  failed,
  children,
}: {
  label: string
  file?: true
  /** Says why it could not be read, in `label`. */
  failed?: true
  /** An action in the tile, in place of its icon. */
  children?: React.ReactNode
}) {
  return (
    <span
      className={`${tile} w-32 flex-col gap-1 px-1.5 text-accent-foreground`}
      data-image-reference
      data-held-file={failed ? "failed" : undefined}
    >
      {children ??
        (file ? (
          <FileIcon aria-hidden="true" className="size-5 text-muted-foreground" />
        ) : (
          <ImageIcon aria-hidden="true" className="size-5 text-muted-foreground" />
        ))}
      <span className="w-full truncate text-center font-sans nessa-text-1">{label}</span>
    </span>
  )
}
