import {
  FilePreview,
  FilePreviewContent,
  FilePreviewHeader,
  FilePreviewFallback,
  detectFileKind,
} from "@nessa-ui/react/file-preview"
import type { FileAttachment } from "../../conversation"

// Text-based strategies parse/highlight and render the whole file. Keep their
// input small until the shared preview offers virtualized or paged rendering.
const MAX_STRUCTURED_PREVIEW_BYTES = 32 * 1024
const structuredKinds = new Set(["text", "markdown", "json", "csv"])

/** Render local file bytes using the design system's kind-specific preview. */
export default function AttachmentPreview({ file }: { file: FileAttachment }) {
  const largeStructuredFile =
    file.size > MAX_STRUCTURED_PREVIEW_BYTES && structuredKinds.has(detectFileKind(file))
  // A file that travels as a path was never read, so there is nothing here to
  // show — and an empty `src` is not nothing, it is a viewer pointed at a file
  // that does not exist, chosen by the name's extension. Say so instead.
  const nothingRead = file.previewUrl === ""
  return (
    <FilePreview
      file={{
        src: file.previewUrl,
        name: file.name,
        mimeType: file.mimeType,
        size: file.size,
      }}
    >
      <FilePreviewHeader />
      {nothingRead ? (
        <FilePreviewFallback message="Nessa has not opened this file. The message says where it is, and the agent reads it from there with your approval." />
      ) : largeStructuredFile ? (
        <FilePreviewFallback message="This file is too large for an inline preview. Download it to view the full contents." />
      ) : (
        <FilePreviewContent />
      )}
    </FilePreview>
  )
}
