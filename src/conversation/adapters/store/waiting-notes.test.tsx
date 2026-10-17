// @vitest-environment jsdom
// The panel's transcript of a view as its follow applies it (`applyView`): the
// order it draws inputs still waiting in, and the notes that followed them.
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { applyView } from "../../application/usecases/apply-view"
import type { ConversationView } from "../../application/view"
import { conversation } from "../../model"
import { ClockProvider } from "../../../clock"
import { Transcript } from "../../ui/transcript"

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
  document.body.appendChild(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

const waiting = (executionId: string, text: string) => ({
  executionId,
  text,
  attachments: [],
  files: [],
  mode: "queued" as const,
})

it("draws an input waiting that the view lists apart from its turns after every turn, its note right after it, as the desktop does", async () => {
  // A bounded view can list a waiting input (p0) without its turn.
  const view: ConversationView = {
    conversationId: "server",
    revision: "1",
    artifacts: [],
    unreadable: [],
    unreadableTruncated: false,
    notes: [
      {
        id: "after-p0",
        text: "Note after-p0",
        author: { kind: "owner", surface: "desktop" },
        deliveryState: "recorded",
        afterExecutionId: "p0",
      },
    ],
    title: null,
    messages: [
      {
        executionId: "e1",
        userText: "First ask",
        attachments: [],
        files: [],
        status: "completed",
        parts: [
          { offset: 0, kind: "text", text: "First answer", toolId: "", noticeId: "" },
        ],
      },
      {
        executionId: "q1",
        userText: "Listed waiting",
        attachments: [],
        files: [],
        status: "queued",
        parts: [],
      },
    ],
    pending: [waiting("p0", "Unlisted waiting"), waiting("q1", "Listed waiting")],
    reviewsWaiting: [],
    questionsWaiting: [],
    permissions: [],
    questions: [],
    tools: [],
    queueComplete: true,
    transcriptState: "complete",
    truncated: true,
    approvalMode: "ask",
    approvalModes: [],
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
  }
  await React.act(async () => {
    root.render(
      <Provider store={store as never}>
        <ClockProvider now={() => 1_000}>
          <Transcript
            conversation={applyView(conversation("tab"), view)}
            ground="paper"
            animateMount={false}
            streamText={false}
            emptyState={false}
            statusLabel="Ready to start"
            gatewayAvailable
            onOpenPaste={vi.fn()}
          />
        </ClockProvider>
      </Provider>,
    )
  })
  const order = [
    ...container.querySelectorAll("[data-note], [data-slot='chat-bubble']"),
  ].map((element) =>
    element.hasAttribute("data-note")
      ? `note:${element.getAttribute("data-note")}`
      : (element.textContent ?? "").trim(),
  )
  // The desktop's order for the same view (`gateway-views.test.ts`).
  expect(order).toEqual([
    "First ask",
    "First answer",
    "Listed waiting",
    "Unlisted waiting",
    "note:after-p0",
  ])
})
