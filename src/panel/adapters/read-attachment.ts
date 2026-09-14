import type { FileAttachment } from "../../conversation"

/** Read a selected local file; bytes remain in the draft and are never uploaded. */
export function readAttachment(file: File): Promise<FileAttachment> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onerror = () => reject(new Error(`Could not read ${file.name}`))
    reader.onabort = () => reject(new Error(`Reading ${file.name} was cancelled`))
    reader.onload = () => {
      if (typeof reader.result !== "string") {
        reject(new Error(`Could not read ${file.name}`))
        return
      }
      resolve({
        type: "file",
        id: crypto.randomUUID(),
        name: file.name,
        mimeType: file.type || "application/octet-stream",
        size: file.size,
        dataUrl: reader.result,
      })
    }
    reader.readAsDataURL(file)
  })
}
