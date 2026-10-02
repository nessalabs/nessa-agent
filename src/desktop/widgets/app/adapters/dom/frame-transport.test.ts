// @vitest-environment jsdom
/**
 * The frame transport (#349, L27): only messages from this frame's window and
 * the sandbox's origin are read, read once into a copy; what is posted goes
 * to that window, addressed to that origin, and nothing goes after close.
 */
import { afterEach, describe, expect, it, vi } from "vitest"
import type { FromFrame } from "../../model/messages"
import { frameTransport } from "./frame-transport"

const origin = "http://127.0.0.1:9999"
const frames: HTMLIFrameElement[] = []

function frame() {
  const made = document.createElement("iframe")
  document.body.append(made)
  frames.push(made)
  return made
}

afterEach(() => {
  for (const made of frames.splice(0)) made.remove()
})

const ping = { jsonrpc: "2.0", id: 1, method: "ping" }
const say = (data: unknown, source: Window | null, from = origin) =>
  window.dispatchEvent(new MessageEvent("message", { data, source, origin: from }))

describe("what is read", () => {
  it("a message from this frame's window and the sandbox's origin, as a typed copy", () => {
    const own = frame()
    const heard: FromFrame[] = []
    const transport = frameTransport(own, origin, (message) => void heard.push(message))
    say(ping, own.contentWindow)
    expect(heard).toEqual([{ kind: "request", id: 1, request: { method: "ping" } }])
    transport.close()
  })

  it("nothing from another frame, the page itself, no window, or another origin — its data untouched", () => {
    const own = frame()
    const other = frame()
    const heard: FromFrame[] = []
    const transport = frameTransport(own, origin, (message) => void heard.push(message))
    const read = vi.fn(() => "ping")
    const watched = { jsonrpc: "2.0", id: 1 }
    Object.defineProperty(watched, "method", { get: read, enumerable: true })
    say(watched, other.contentWindow)
    say(watched, window)
    say(watched, null)
    say(watched, own.contentWindow, "http://127.0.0.1:1420")
    say(watched, own.contentWindow, "null")
    expect(heard).toEqual([])
    expect(read).not.toHaveBeenCalled()
    transport.close()
  })

  it("nothing after close", () => {
    const own = frame()
    const heard: FromFrame[] = []
    const transport = frameTransport(own, origin, (message) => void heard.push(message))
    transport.close()
    say(ping, own.contentWindow)
    expect(heard).toEqual([])
  })
})

describe("what is posted", () => {
  it("to this frame's window, addressed to the sandbox's origin, and nothing after close", () => {
    const own = frame()
    const post = vi.fn()
    own.contentWindow!.postMessage = post as typeof window.postMessage
    const transport = frameTransport(own, origin, () => {})
    const message = { jsonrpc: "2.0", id: 1, result: {} } as const
    transport.post(message)
    expect(post).toHaveBeenCalledWith(message, origin)
    transport.close()
    transport.post(message)
    expect(post).toHaveBeenCalledTimes(1)
  })
})
