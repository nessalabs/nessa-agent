import { expect, it } from "vitest"
import { pastedFiles } from "./pasted-files"

/** A paste's clipboard as an engine hands it over: its text, files and items. */
function clipboard({
  text = "",
  files = [],
  items = [],
}: {
  text?: string
  files?: File[]
  items?: { kind: string; file: File | null }[]
}): DataTransfer {
  return {
    getData: (type: string) => (type === "text/plain" ? text : ""),
    files,
    items: items.map(({ kind, file }) => ({ kind, getAsFile: () => file })),
  } as unknown as DataTransfer
}

const shot = new File([new Uint8Array([1])], "image.png", { type: "image/png" })

it("reads a copied screenshot from the files, or the items where an engine lists it only there", () => {
  expect(pastedFiles(clipboard({ files: [shot] }))).toEqual([shot])
  expect(
    pastedFiles(
      clipboard({
        items: [
          { kind: "string", file: null },
          { kind: "file", file: shot },
        ],
      }),
    ),
  ).toEqual([shot])
})

it("takes a paste with text in it as text, and nothing at all as no files", () => {
  expect(pastedFiles(clipboard({ text: "A1\tB1", files: [shot] }))).toEqual([])
  expect(pastedFiles(clipboard({}))).toEqual([])
  expect(pastedFiles(null)).toEqual([])
})
