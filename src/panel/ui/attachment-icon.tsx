import { File, FileArchive, FileCode, FileText, Film, Music, Sheet } from "lucide-react"

/** Uses the existing Lucide set for file categories; images use their thumbnail. */
export function AttachmentIcon({ name, mimeType }: { name: string; mimeType: string }) {
  const extension = name.split(".").pop()?.toLowerCase() ?? ""
  const Icon = mimeType.startsWith("audio/")
    ? Music
    : mimeType.startsWith("video/")
      ? Film
      : ["zip", "gz", "tar", "rar", "7z"].includes(extension)
        ? FileArchive
        : ["csv", "xls", "xlsx", "ods"].includes(extension)
          ? Sheet
          : [
                "js",
                "jsx",
                "ts",
                "tsx",
                "json",
                "html",
                "css",
                "py",
                "rs",
                "sh",
                "xml",
                "yaml",
                "yml",
              ].includes(extension)
            ? FileCode
            : mimeType.startsWith("text/") ||
                ["pdf", "doc", "docx", "md", "txt", "rtf"].includes(extension)
              ? FileText
              : File
  return <Icon aria-hidden="true" />
}
