// @vitest-environment jsdom
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { textContent, type Conversation } from "../model"
import { Transcript } from "./transcript"

class TestResizeObserver {
  observe() {}
  disconnect() {}
}

const store = {
  dispatch: vi.fn(),
  getState: () => ({}),
  subscribe: () => () => {},
  replaceReducer: vi.fn(),
}

function gapped(): Conversation {
  return {
    id: "gap",
    title: "Gap",
    phase: "idle",
    draft: [],
    turns: [
      {
        id: "before:user",
        from: "user",
        executionId: "before",
        receipt: "delivered",
        content: textContent("Kept before"),
      },
      {
        id: "before:assistant",
        from: "assistant",
        executionId: "before",
        text: "Answer before",
        status: "completed",
        parts: [{ offset: 0, kind: "text", text: "Answer before", toolId: "", noticeId: "" }],
      },
      {
        id: "after:user",
        from: "user",
        executionId: "after",
        receipt: "delivered",
        content: textContent("Kept after"),
      },
      {
        id: "after:assistant",
        from: "assistant",
        executionId: "after",
        text: "Answer after",
        status: "completed",
        parts: [{ offset: 0, kind: "text", text: "Answer after", toolId: "", noticeId: "" }],
      },
    ],
    remote: {
      approvalMode: "ask",
      approvalModes: [
        { id: "ask", name: "Ask", description: "Ask before tools." },
      ],
      questions: [],
      running: false,
      permissions: [],
      tools: [],
      pending: [],
      capabilities: {
        queue: true,
        steer: true,
        resume: true,
        permissions: true,
        imageInput: false,
        agentFeatures: {
          permissionDenial: "unknown",
          nativeHookSuppression: "unknown",
          compactionReporting: "unsupported_not_implemented",
          modelSwitchReporting: "unsupported_not_implemented",
          permissionDeferral: "unsupported_not_implemented",
          elicitationForwarding: "unknown",
          preToolPolicy: "unsupported_not_implemented",
          policyEndTurn: "unsupported_not_implemented",
          policyCloseSession: "unsupported_not_implemented",
          incomingElicitation: "unsupported",
        },
      },
      lifecycle: { phase: "attached" },
      queueComplete: true,
      transcriptState: "complete",
      truncated: false,
      unreadable: [
        {
          session: "gap",
          position: 4,
          reason: "another_version",
          found: 2,
          afterTurnId: "before:user",
        },
      ],
    },
  }
}

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  globalThis.ResizeObserver = TestResizeObserver as unknown as typeof ResizeObserver
  window.matchMedia = vi.fn().mockReturnValue({
    matches: false,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  })
  Element.prototype.animate = vi.fn(() => ({ cancel: vi.fn() })) as never
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

it("draws a muted row for a part it could not read, between the messages around it", async () => {
  await React.act(async () => {
    root.render(
      <Provider store={store as never}>
        <Transcript
          conversation={gapped()}
          ground="paper"
          animateMount={false}
          streamText={false}
          emptyState
          statusLabel="Ready"
          gatewayAvailable
          onOpenPaste={vi.fn()}
        />
      </Provider>,
    )
  })
  const row = container.querySelector("[data-unreadable-position]")
  expect(row?.textContent).toBe("Couldn't read this part of the conversation")
  expect(row?.querySelector("button")).toBeNull()
  expect(row?.getAttribute("data-unreadable-session")).toBe("gap")
  expect(row?.getAttribute("data-unreadable-position")).toBe("4")
  expect(row?.getAttribute("data-unreadable-reason")).toBe("another_version")
  expect(row?.getAttribute("data-unreadable-found")).toBe("2")
  const text = container.textContent ?? ""
  const gap = text.indexOf("Couldn't read this part of the conversation")
  expect(text.indexOf("Kept before")).toBeLessThan(gap)
  expect(text.indexOf("Answer before")).toBeLessThan(gap)
  expect(gap).toBeLessThan(text.indexOf("Kept after"))
  expect(gap).toBeLessThan(text.indexOf("Answer after"))
})
