import type {
  FileAttachment,
  ImageReference,
  ImageReferencePart,
  LinkedFile,
  LinkedFileReferencePart,
} from "./attachments"

/** Ordered message content; pasted payloads remain literal and independently viewable. */
export type MessagePart =
  | { type: "text"; text: string }
  | { type: "pasted-text"; id: string; text: string }
  | FileAttachment
  | ImageReferencePart
  | LinkedFileReferencePart

export type MessageContent = MessagePart[]

/**
 * Text sent to the agent, retaining the exact order and whitespace of pasted
 * payloads. Files and images contribute nothing here: they travel as references.
 */
export function contentText(content: MessageContent): string {
  return content
    .map((part) => (part.type === "text" || part.type === "pasted-text" ? part.text : ""))
    .join("")
}

/** Creates an ordinary Markdown draft. */
export function textContent(text: string): MessageContent {
  return text ? [{ type: "text", text }] : []
}

/**
 * A message as the gateway reports it: its text, its images by reference, then
 * the files it pointed the agent at. This window never held any of those bytes
 * — and for a linked file nobody did — so the parts carry nothing to preview.
 */
export function referencedContent(
  text: string,
  images: readonly ImageReference[],
  files: readonly LinkedFile[] = [],
): MessageContent {
  return [
    ...textContent(text),
    ...images.map((image) => ({
      type: "image-reference" as const,
      digest: image.digest,
      mimeType: image.mimeType,
      size: image.size,
    })),
    ...files.map((file) => ({ type: "file-reference" as const, path: file.path })),
  ]
}
