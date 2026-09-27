import { expect, it, vi } from "vitest"
import {
  droppedFolderEntries,
  FolderDropEmptyError,
  FolderDropLimitError,
  readDroppedFolder,
} from "./dropped-folder"

function fileEntry(name: string) {
  const file = vi.fn((resolve: (file: File) => void) => resolve(new File([name], name)))
  return { isFile: true, isDirectory: false, file } as unknown as FileSystemFileEntry
}

function folderEntry(batches: FileSystemEntry[][]) {
  let index = 0
  return {
    isFile: false,
    isDirectory: true,
    createReader: () => ({
      readEntries: (resolve: (entries: FileSystemEntry[]) => void) =>
        resolve(batches[index++] ?? []),
    }),
  } as unknown as FileSystemDirectoryEntry
}

it("reads nested and paginated folders in order", async () => {
  const entries = [
    folderEntry([
      [fileEntry("one"), folderEntry([[fileEntry("two")]])],
      [fileEntry("three")],
    ]),
  ]
  expect(
    (await readDroppedFolder(entries, 20, new AbortController().signal)).map(
      (file) => file.name,
    ),
  ).toEqual(["one", "two", "three"])
})

it("rejects the complete batch before reading files beyond the limit", async () => {
  const excess = fileEntry("excess")
  await expect(
    readDroppedFolder(
      [folderEntry([[fileEntry("one"), excess]])],
      1,
      new AbortController().signal,
    ),
  ).rejects.toBeInstanceOf(FolderDropLimitError)
  expect(excess.file).not.toHaveBeenCalled()
})

it("bounds traversal even when every directory is empty", async () => {
  await expect(
    readDroppedFolder(
      Array.from({ length: 1001 }, () => folderEntry([])),
      20,
      new AbortController().signal,
    ),
  ).rejects.toBeInstanceOf(FolderDropLimitError)
})

it("stops reading after cancellation", async () => {
  const controller = new AbortController()
  controller.abort()
  const file = fileEntry("unread")
  await expect(readDroppedFolder([file], 20, controller.signal)).rejects.toMatchObject({
    name: "AbortError",
  })
  expect(file.file).not.toHaveBeenCalled()
})

it("captures mixed drops but leaves plain files to the shared component", () => {
  const file = fileEntry("one")
  const folder = folderEntry([])
  const transfer = (entries: FileSystemEntry[]) =>
    ({
      items: entries.map((entry) => ({ kind: "file", webkitGetAsEntry: () => entry })),
    }) as unknown as DataTransfer
  expect(droppedFolderEntries(transfer([file]))).toBeNull()
  expect(droppedFolderEntries(transfer([file, folder]))).toEqual([file, folder])
})

it("ignores text items and browsers without entry access", () => {
  const readText = vi.fn()
  expect(
    droppedFolderEntries({
      items: [{ kind: "string", webkitGetAsEntry: readText }, { kind: "file" }],
    } as unknown as DataTransfer),
  ).toBeNull()
  expect(readText).not.toHaveBeenCalled()
})

it("reports empty nested folders instead of silently accepting an empty batch", async () => {
  await expect(
    readDroppedFolder(
      [folderEntry([[folderEntry([])]])],
      20,
      new AbortController().signal,
    ),
  ).rejects.toBeInstanceOf(FolderDropEmptyError)
})
