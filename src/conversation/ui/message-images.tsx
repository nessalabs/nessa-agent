import { Image as ImageIcon } from "lucide-react"
import {
  imageReferenceLabel,
  isImageFile,
  previewableImage,
  type MessageContent,
} from "../model"

// The composer's attachment tile, as a shape: square, rounded, on the accent wash.
const tile = "flex h-16 items-center justify-center overflow-hidden rounded-xl bg-accent"

/**
 * The images a sent turn carried, as tiles under its text.
 *
 * A turn sent from this window still has its object URL, so its tile is the
 * picture as it was attached — the original, not the copy the gateway stored,
 * which this window never sees. A turn read back from the gateway — after a
 * reload, or sent from another surface — is known only by reference, and gets a
 * labelled placeholder saying what it was and how big. Reading the bytes back from the gateway to
 * paint that tile is deliberately not done here; nothing fetches by digest yet.
 *
 * Renders nothing for a turn with no images, so a text turn's markup is
 * unchanged. An image-only turn renders only this, which is why every tile has
 * a name a screen reader and a pointer can both reach, not only a picture.
 */
export function MessageImages({ content }: { content: MessageContent }) {
  type Tile = { key: string; label: string; src: string | undefined }
  const tiles = content.flatMap((part, index): Tile[] => {
    if (part.type === "file" && isImageFile(part.mimeType))
      return [
        {
          key: part.id,
          label: part.name,
          // An original the webview cannot paint (HEIC, RAW) gets the labelled
          // tile too, by its file name, rather than a broken picture.
          src: previewableImage(part.mimeType) ? part.previewUrl : undefined,
        },
      ]
    if (part.type === "image-reference")
      return [
        {
          // The same image may be sent twice in one message; position disambiguates.
          key: `${part.digest}:${index}`,
          label: imageReferenceLabel(part.mimeType, part.size),
          src: undefined,
        },
      ]
    return []
  })
  if (tiles.length === 0) return null
  return (
    <ul
      aria-label={tiles.length === 1 ? "1 image" : `${tiles.length} images`}
      className="nessa-message-images m-0 flex list-none flex-wrap gap-1.5 p-0 [&:not(:first-child)]:mt-2"
    >
      {tiles.map((item) => (
        <li key={item.key} title={item.label} className="inline-flex">
          {item.src ? (
            <span className={`${tile} w-16`}>
              <img src={item.src} alt={item.label} className="size-full object-cover" />
            </span>
          ) : (
            <span
              className={`${tile} w-32 flex-col gap-1 px-1.5 text-accent-foreground`}
              data-image-reference
            >
              <ImageIcon aria-hidden="true" className="size-5 text-muted-foreground" />
              <span className="w-full truncate text-center font-sans nessa-text-1">
                {item.label}
              </span>
            </span>
          )}
        </li>
      ))}
    </ul>
  )
}
