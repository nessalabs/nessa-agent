import {
  FilePreview,
  FilePreviewContent,
  FilePreviewHeader,
} from "@nessa-ui/react/file-preview"
import type { FileAttachment } from "../../conversation"

/** Render local file bytes using the design system's kind-specific preview. */
export default function AttachmentPreview({ file }: { file: FileAttachment }) {
  return (
    <FilePreview
      file={{
        src: file.dataUrl,
        name: file.name,
        mimeType: file.mimeType,
        size: file.size,
      }}
    >
      <FilePreviewHeader />
      <FilePreviewContent />
    </FilePreview>
  )
}
