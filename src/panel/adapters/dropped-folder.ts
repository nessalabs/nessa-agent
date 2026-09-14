/** Capture entries during the drop event, before its data store is cleared. */
export function droppedFolderEntries(transfer: DataTransfer): FileSystemEntry[] | null {
  const entries = Array.from(transfer.items, (item) =>
    item.kind === "file" && typeof item.webkitGetAsEntry === "function"
      ? item.webkitGetAsEntry()
      : null,
  ).filter((entry): entry is FileSystemEntry => entry !== null)
  return entries.some((entry) => entry.isDirectory) ? entries : null
}

export class FolderDropEmptyError extends Error {
  constructor() {
    super("The dropped folders contain no files.")
  }
}

export class FolderDropLimitError extends Error {
  constructor() {
    super("The dropped folders exceed the attachment or folder scan limit.")
  }
}

/** Sequential traversal bounds file handles and work even for mostly empty trees. */
export async function readDroppedFolder(
  entries: readonly FileSystemEntry[],
  maxFiles: number,
  signal: AbortSignal,
): Promise<File[]> {
  const files: File[] = []
  let examined = 0
  async function visit(entry: FileSystemEntry): Promise<void> {
    signal.throwIfAborted()
    if (++examined > 1000) throw new FolderDropLimitError()
    if (entry.isFile) {
      if (files.length >= maxFiles) throw new FolderDropLimitError()
      const file = await new Promise<File>((resolve, reject) =>
        (entry as FileSystemFileEntry).file(resolve, reject),
      )
      signal.throwIfAborted()
      files.push(file)
    } else if (entry.isDirectory) {
      const reader = (entry as FileSystemDirectoryEntry).createReader()
      for (;;) {
        signal.throwIfAborted()
        const children = await new Promise<FileSystemEntry[]>((resolve, reject) =>
          reader.readEntries(resolve, reject),
        )
        if (!children.length) break
        for (const child of children) await visit(child)
      }
    }
  }
  for (const entry of entries) await visit(entry)
  if (!files.length) throw new FolderDropEmptyError()
  return files
}
