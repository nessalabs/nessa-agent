// @vitest-environment jsdom
import { act, useCallback } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { useStickToBottom } from "./use-stick-to-bottom"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

function Harness({ identity, signature }: { identity: string; signature: string }) {
  const stick = useStickToBottom(identity, signature)
  const attach = useCallback(
    (node: HTMLDivElement | null) => {
      if (node) {
        Object.defineProperty(node, "scrollHeight", { configurable: true, value: 500 })
        Object.defineProperty(node, "clientHeight", { configurable: true, value: 100 })
      }
      stick.ref.current = node
    },
    [stick.ref],
  )
  return (
    <div data-scroll ref={attach} onScroll={stick.onScroll}>
      <button type="button" data-slot="tool-trace-line">
        <span>Ran 1 tool</span>
      </button>
    </div>
  )
}

function scroller(): HTMLDivElement {
  const node = host.querySelector<HTMLDivElement>("[data-scroll]")
  if (!node) throw new Error("no scroller")
  return node
}

describe("useStickToBottom", () => {
  it("follows a growth while the reader is at the end, and stays put when they are not", async () => {
    await act(async () => root.render(<Harness identity="mara" signature="1" />))
    const node = scroller()
    expect(node.scrollTop).toBe(500)

    node.scrollTop = 0
    await act(async () => {
      node.dispatchEvent(new Event("scroll"))
    })
    await act(async () => root.render(<Harness identity="mara" signature="2" />))
    expect(node.scrollTop).toBe(0)

    node.scrollTop = 370
    await act(async () => {
      node.dispatchEvent(new Event("scroll"))
    })
    await act(async () => root.render(<Harness identity="mara" signature="3" />))
    expect(node.scrollTop).toBe(500)
  })

  it("holds the end while a tool disclosure is open, then follows a later growth at the end", async () => {
    const frames: FrameRequestCallback[] = []
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
      frames.push(callback)
      return frames.length
    })
    vi.spyOn(window, "cancelAnimationFrame").mockImplementation(() => {})
    const flush = async () => {
      const due = frames.splice(0)
      await act(async () => {
        for (const frame of due) frame(0)
      })
    }
    await act(async () => root.render(<Harness identity="mara" signature="1" />))
    const node = scroller()
    expect(node.scrollTop).toBe(500)
    await act(async () => {
      node
        .querySelector("span")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }))
    })
    expect(node.hasAttribute("data-disclosure-hold")).toBe(true)
    Object.defineProperty(node, "scrollHeight", { configurable: true, value: 800 })
    await act(async () => root.render(<Harness identity="mara" signature="2" />))
    expect(node.scrollTop).toBe(500)
    await flush()
    await flush()
    expect(node.hasAttribute("data-disclosure-hold")).toBe(false)
    Object.defineProperty(node, "scrollHeight", { configurable: true, value: 900 })
    await act(async () => root.render(<Harness identity="mara" signature="3" />))
    expect(node.scrollTop).toBe(500)
    node.scrollTop = 800
    await act(async () => node.dispatchEvent(new Event("scroll")))
    await act(async () => root.render(<Harness identity="mara" signature="4" />))
    expect(node.scrollTop).toBe(900)
    vi.restoreAllMocks()
  })

  it("starts pinned again for another child", async () => {
    await act(async () => root.render(<Harness identity="mara" signature="1" />))
    const node = scroller()
    node.scrollTop = 0
    await act(async () => {
      node.dispatchEvent(new Event("scroll"))
    })
    await act(async () => root.render(<Harness identity="idris" signature="1" />))
    expect(scroller().scrollTop).toBe(500)
  })
})
