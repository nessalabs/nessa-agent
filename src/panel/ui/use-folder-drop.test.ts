// @vitest-environment jsdom
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

const { readDroppedFolder } = vi.hoisted(() => ({ readDroppedFolder: vi.fn() }))
vi.mock("../adapters/dropped-folder", () => ({
  FolderDropEmptyError: class FolderDropEmptyError extends Error {},
  FolderDropLimitError: class FolderDropLimitError extends Error {},
  readDroppedFolder,
}))

import type { AttachmentDestination } from "./use-file-attachments"
import { useFolderDrop } from "./use-folder-drop"

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => (resolve = done))
  return { promise, resolve }
}

const destination = (conversationId: string): AttachmentDestination => ({
  store: {} as AttachmentDestination["store"],
  conversationId,
  resources: {} as AttachmentDestination["resources"],
  attachFiles: vi.fn(),
})

let container: HTMLDivElement
let root: Root
let selected: AttachmentDestination
let hook: ReturnType<typeof useFolderDrop>
const addFiles = vi.fn()
const refuse = vi.fn()

function Surface() {
  hook = useFolderDrop(selected, addFiles, refuse)
  return null
}

beforeEach(() => {
  readDroppedFolder.mockReset()
  addFiles.mockReset()
  refuse.mockReset()
  selected = destination("A")
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await act(async () => root.unmount())
  container.remove()
})

it("retains A's folder operation when B finishes first", async () => {
  const readA = deferred<File[]>()
  const readB = deferred<File[]>()
  const signals: AbortSignal[] = []
  readDroppedFolder.mockImplementation(
    (_entries: FileSystemEntry[], _limit: number, signal: AbortSignal) => {
      signals.push(signal)
      return signals.length === 1 ? readA.promise : readB.promise
    },
  )
  const destinationA = selected
  await act(async () => root.render(React.createElement(Surface)))
  await act(async () => hook.addFolderEntries([]))

  const destinationB = destination("B")
  selected = destinationB
  await act(async () => root.render(React.createElement(Surface)))
  await act(async () => hook.addFolderEntries([]))
  expect(signals).toHaveLength(2)

  const fileA = new File(["a"], "A.txt")
  const fileB = new File(["b"], "B.txt")
  await act(async () => {
    readB.resolve([fileB])
    await Promise.resolve()
  })
  expect(addFiles).toHaveBeenCalledWith([fileB], destinationB)

  selected = destinationA
  await act(async () => root.render(React.createElement(Surface)))
  expect(hook.isPending("A")).toBe(true)
  expect(hook.isPending("B")).toBe(false)

  await act(async () => {
    readA.resolve([fileA])
    await Promise.resolve()
  })
  expect(addFiles).toHaveBeenCalledWith([fileA], destinationA)
})

it("aborts every active folder operation when the hook unmounts", async () => {
  const reads = [deferred<File[]>(), deferred<File[]>()]
  const signals: AbortSignal[] = []
  readDroppedFolder.mockImplementation(
    (_entries: FileSystemEntry[], _limit: number, signal: AbortSignal) => {
      signals.push(signal)
      return reads[signals.length - 1]!.promise
    },
  )
  await act(async () => root.render(React.createElement(Surface)))
  await act(async () => hook.addFolderEntries([]))
  selected = destination("B")
  await act(async () => root.render(React.createElement(Surface)))
  await act(async () => hook.addFolderEntries([]))

  await act(async () => root.unmount())
  expect(signals).toHaveLength(2)
  expect(signals.every((signal) => signal.aborted)).toBe(true)
})
