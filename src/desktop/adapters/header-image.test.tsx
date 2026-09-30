// @vitest-environment jsdom
/**
 * The header picture is the window's one: a picture chosen in the home header
 * reaches every pane's sliver at once, and a GIF moves only where it is asked
 * to — elsewhere its first frame is shown, drawn once — so unfocused panes do
 * no per-frame work.
 */
import { act } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import {
  useChooseHeaderPicture,
  useHeaderImage,
  useHeaderPictureUrl,
} from "./header-image"

let host: HTMLDivElement
let made = 0

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  made = 0
  URL.createObjectURL = () => `blob:picture-${++made}`
  URL.revokeObjectURL = () => {}
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => host.remove())

let choose: (blob: Blob) => void = () => {}
function Chooser() {
  choose = useHeaderImage()[1]
  return null
}
function Shown({ still, id }: { still: boolean; id: string }) {
  const url = useHeaderPictureUrl(still)
  return <i data-id={id} data-url={url ?? ""} />
}
const urlOf = (id: string) =>
  host.querySelector(`[data-id="${id}"]`)?.getAttribute("data-url")

it("shows a picture chosen in one place everywhere, from one file", async () => {
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <>
        <Chooser />
        <Shown id="focused" still={false} />
        <Shown id="other" still />
      </>,
    ),
  )
  await act(async () => choose(new Blob(["png"], { type: "image/png" })))
  await act(async () => new Promise((resolve) => setTimeout(resolve, 0)))
  expect(urlOf("focused")).toBe("blob:picture-1")
  // A still picture is its own first frame: the same file, not a copy.
  expect(urlOf("other")).toBe("blob:picture-1")
  expect(made).toBe(1)
  await act(async () => root.unmount())
})

it("shows a GIF's first frame, drawn once, wherever it is not to move", async () => {
  const drawn: string[] = []
  Object.assign(globalThis, {
    createImageBitmap: async () => ({ width: 4, height: 2, close() {} }),
  })
  HTMLCanvasElement.prototype.getContext = (() => ({
    drawImage: () => drawn.push("frame"),
  })) as never
  HTMLCanvasElement.prototype.toBlob = function (done: BlobCallback) {
    done(new Blob(["still"], { type: "image/png" }))
  }
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <>
        <Chooser />
        <Shown id="focused" still={false} />
        <Shown id="a" still />
        <Shown id="b" still />
      </>,
    ),
  )
  await act(async () => choose(new Blob(["gif"], { type: "image/gif" })))
  for (let tick = 0; tick < 5; tick++)
    await act(async () => new Promise((resolve) => setTimeout(resolve, 0)))
  const moving = urlOf("focused")
  expect(moving).toMatch(/^blob:picture-/)
  expect(urlOf("a")).not.toBe(moving)
  expect(urlOf("a")).toMatch(/^blob:picture-/)
  expect(urlOf("b")).toBe(urlOf("a"))
  expect(drawn).toEqual(["frame"])
  await act(async () => root.unmount())
})

/** Picks `file` in the file dialog that `ask` opens. */
async function pick(ask: () => void, file: File) {
  // The input the dialog belongs to, made as it is asked for; its click opens nothing here.
  const made: HTMLInputElement[] = []
  const create = document.createElement.bind(document)
  const creating = vi
    .spyOn(document, "createElement")
    .mockImplementation((tag: string, options?: ElementCreationOptions) => {
      const element = create(tag, options)
      if (element instanceof HTMLInputElement) {
        element.click = () => {}
        made.push(element)
      }
      return element
    })
  try {
    await act(async () => ask())
  } finally {
    creating.mockRestore()
  }
  const input = made[0]
  expect(input?.type).toBe("file")
  expect(input?.accept).toBe("image/*")
  Object.defineProperty(input, "files", { value: [file] })
  await act(async () => {
    input?.dispatchEvent(new Event("change"))
  })
}

it("asks for a picture the one way, wherever it is asked from: taken, or refused with why", async () => {
  const heard: string[] = []
  let ask: () => void = () => {}
  let clear: () => void = () => {}
  function Asker() {
    clear = useHeaderImage()[2]
    ask = useChooseHeaderPicture({
      onChosen: () => heard.push("chosen"),
      onRefused: (reason) => heard.push(reason),
    })
    return null
  }
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <>
        <Asker />
        <Shown id="shown" still={false} />
      </>,
    ),
  )
  // The window's one picture outlives a test; this one starts from the scene.
  await act(async () => clear())
  await pick(() => ask(), new File(["text"], "notes.txt", { type: "text/plain" }))
  expect(heard).toEqual(["not-an-image"])
  expect(urlOf("shown")).toBe("")
  await pick(() => ask(), new File(["png"], "sky.png", { type: "image/png" }))
  await act(async () => new Promise((resolve) => setTimeout(resolve, 0)))
  expect(heard).toEqual(["not-an-image", "chosen"])
  expect(urlOf("shown")).not.toBe("")
  await act(async () => root.unmount())
})
