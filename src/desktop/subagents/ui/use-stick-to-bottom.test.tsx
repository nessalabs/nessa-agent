// @vitest-environment jsdom
import { act, useCallback } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
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
  return <div data-scroll ref={attach} onScroll={stick.onScroll} />
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
