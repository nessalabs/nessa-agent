// @vitest-environment jsdom
import { act } from "react"
import { createRoot } from "react-dom/client"
import { beforeEach, expect, it, vi } from "vitest"
import { HeldFilesProvider } from "../../../../conversation"
import { transcriptFrom } from "../../adapters/gateway/gateway-views"
import { view } from "../../adapters/gateway/fake-gateway"
import { Message } from "./message"

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
})

it("shows retained input metadata, reading back only the image the conversation holds", async () => {
  // Still on its way: the tile is its labelled placeholder meanwhile.
  const read = vi.fn(() => new Promise<Blob>(() => {}))
  const save = vi.fn(async () => "cancelled" as const)
  const transcript = transcriptFrom(
    view("original", {
      messages: [
        {
          executionId: "e1",
          userText: "Inspect `this` [report].",
          attachments: [{ digest: "a".repeat(64), mimeType: "image/png", size: 2048 }],
          files: [{ path: "/workspace/quarterly [report].csv" }],
          status: "completed",
          parts: [],
        },
      ],
    }),
    1,
    () => 1,
    1000,
  )
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  try {
    await act(async () =>
      root.render(
        <HeldFilesProvider read={read} save={save}>
          <Message sessionId="original" message={transcript.messages[0]} isNew={false} />
        </HeldFilesProvider>,
      ),
    )
    expect(host.querySelector('ul[aria-label="2 attachments"]')).not.toBeNull()
    expect([...host.querySelectorAll("li")].map((tile) => tile.title)).toEqual([
      "PNG image, 2 KiB",
      "/workspace/quarterly [report].csv",
    ])
    expect(host.textContent).toContain("Inspect this [report].")
    expect(host.querySelector("img, button")).toBeNull()
    expect(read).toHaveBeenCalledTimes(1)
    expect(read.mock.calls[0]).toEqual([
      "original",
      { digest: "a".repeat(64), mimeType: "image/png", size: 2048 },
      expect.anything(),
    ])
    expect(save).not.toHaveBeenCalled()
  } finally {
    await act(async () => root.unmount())
    host.remove()
  }
})
