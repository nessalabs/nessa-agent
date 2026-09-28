// @vitest-environment jsdom
/**
 * The header picture is the window's one: a picture chosen in the home header
 * reaches every pane's sliver at once, and a GIF moves only where it is asked
 * to — elsewhere its first frame is shown, drawn once — so unfocused panes do
 * no per-frame work.
 */
import { act } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { useHeaderImage, useHeaderPictureUrl } from "./header-image"

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
