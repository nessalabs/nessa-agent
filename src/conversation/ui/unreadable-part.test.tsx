// @vitest-environment jsdom
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { textContent, type Conversation, type Turn } from "../model"
import { ClockProvider } from "../../clock"
import { Transcript } from "./transcript"
import { UNREADABLE_MORE_SENTENCE, UNREADABLE_SENTENCE } from "./unreadable-part"

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

function conversation(
  unreadable: { id: string; afterExecutionId?: string }[],
  turns: Turn[],
  unreadableTruncated = false,
): Conversation {
  return {
    id: "tab",
    serverConversationId: "server",
    title: "Gaps",
    phase: "idle",
    draft: [],
    turns,
    remote: {
      artifacts: [],
      notes: [],
      unreadable,
      unreadableTruncated,
      approvalMode: "ask",
      approvalModes: [{ id: "ask", name: "Ask", description: "Asks." }],
      questions: [],
      running: false,
      reviewsWaiting: [],
      questionsWaiting: [],
      permissions: [],
      tools: [],
      pending: [],
      capabilities: {
        queue: true,
        steer: true,
        resume: true,
        permissions: true,
        imageInput: true,
        agentFeatures: {
          permissionDenial: "unknown",
          nativeHookSuppression: "unknown",
          nativeHooks: "off",
          compactionReporting: "unsupported_not_implemented",
          modelSwitchReporting: "unsupported_not_implemented",
          permissionDeferral: "unsupported_not_implemented",
          elicitationForwarding: "unknown",
          preToolPolicy: "unsupported_not_implemented",
          policyEndTurn: "unsupported",
          policyCloseSession: "unsupported",
          incomingElicitation: "unsupported",
        },
      },
      lifecycle: { phase: "attached" },
      queueComplete: true,
      transcriptState: turns.length === 0 ? "complete_empty" : "complete",
      truncated: false,
    },
  }
}

const turn = (executionId: string, asked: string, said: string): Turn[] => [
  {
    id: `${executionId}:user`,
    from: "user",
    executionId,
    receipt: "delivered",
    content: textContent(asked),
  },
  {
    id: `${executionId}:assistant`,
    from: "assistant",
    executionId,
    text: said,
    status: "completed",
    parts: [{ offset: 0, kind: "text", text: said, toolId: "", noticeId: "" }],
  },
]

let container: HTMLDivElement
let root: Root

async function render(value: Conversation, emptyState = false) {
  await React.act(async () => {
    root.render(
      <Provider store={store as never}>
        <ClockProvider now={() => 1_000}>
          <Transcript
            conversation={value}
            ground="paper"
            animateMount={false}
            streamText={false}
            emptyState={emptyState}
            statusLabel="Ready to start"
            gatewayAvailable
            onOpenPaste={vi.fn()}
          />
        </ClockProvider>
      </Provider>,
    )
  })
}

function order() {
  return [
    ...container.querySelectorAll(
      "[data-unreadable], [data-unreadable-overflow], [data-slot='chat-bubble']",
    ),
  ].map((element) =>
    element.hasAttribute("data-unreadable-overflow")
      ? "more"
      : element.hasAttribute("data-unreadable")
        ? `gap:${element.getAttribute("data-unreadable")}`
        : (element.textContent ?? "").trim(),
  )
}

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
  document.body.appendChild(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

it("shows a record it could not read as a line, and the rest of the conversation around it", async () => {
  await render(
    conversation(
      [{ id: "3" }, { id: "9", afterExecutionId: "one" }],
      turn("one", "Plan the release.", "Here is the plan."),
    ),
  )
  expect(order()).toEqual(["gap:3", "Plan the release.", "Here is the plan.", "gap:9"])
  const lines = [...container.querySelectorAll("[data-unreadable]")]
  expect(lines.map((line) => line.textContent)).toEqual([
    UNREADABLE_SENTENCE,
    UNREADABLE_SENTENCE,
  ])
  for (const line of lines) {
    expect(line.getAttribute("data-unreadable-report")).toBe("")
    expect(line.querySelector("button")).toBeNull()
  }
})

it("ends with one line when more records could not be read than the transcript lists", async () => {
  await render(
    conversation(
      [{ id: "3" }, { id: "9", afterExecutionId: "one" }],
      turn("one", "Plan the release.", "Here is the plan."),
      true,
    ),
  )
  expect(order()).toEqual([
    "gap:3",
    "Plan the release.",
    "Here is the plan.",
    "gap:9",
    "more",
  ])
  const overflow = container.querySelectorAll("[data-unreadable-overflow]")
  expect(overflow).toHaveLength(1)
  expect(overflow[0]?.textContent).toBe(UNREADABLE_MORE_SENTENCE)
  expect(overflow[0]?.querySelector("button")).toBeNull()
})

it("hides the empty welcome when the gap list was cut and no line was kept", async () => {
  await render(conversation([], [], true), true)
  expect(container.textContent).toContain(UNREADABLE_MORE_SENTENCE)
  expect(container.textContent).not.toContain("Ready to start")
  expect(container.querySelector("[data-unreadable-overflow]")).not.toBeNull()
})

it("shows the line when the chat has no turn, and not the empty welcome", async () => {
  await render(conversation([{ id: "4" }], []), true)
  expect(container.textContent).toContain(UNREADABLE_SENTENCE)
  expect(container.textContent).not.toContain("Ready to start")
  expect(container.querySelector("[data-unreadable='4']")).not.toBeNull()
})
