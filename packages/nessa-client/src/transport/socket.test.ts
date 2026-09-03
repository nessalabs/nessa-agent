import { describe, expect, it } from "vitest"

import { waitForSocketOpen } from "./socket.js"

describe("waitForSocketOpen", () => {
  it("rejects when the socket closes before open", async () => {
    const handlers = new Map<string, Set<() => void>>()
    const socket = {
      readyState: 0,
      addEventListener(type: string, handler: () => void) {
        const set = handlers.get(type) ?? new Set()
        set.add(handler)
        handlers.set(type, set)
      },
      removeEventListener(type: string, handler: () => void) {
        handlers.get(type)?.delete(handler)
      },
      close() {
        for (const handler of handlers.get("close") ?? []) handler()
      },
    } as unknown as WebSocket

    const pending = waitForSocketOpen(socket, 500)
    socket.close()
    await expect(pending).rejects.toThrow("WebSocket closed before open")
  })
})
