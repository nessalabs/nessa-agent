// @vitest-environment jsdom
/** Recovery ordering through local commands, authoritative replacement and actual panel rendering. */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { applyView } from "../../application/usecases/apply-view"
import { beginSend, failSend, reattemptSend } from "../../application/usecases/send-draft"
import { emptyLocalTabs } from "../../application/local-tabs"
import type { ConversationView } from "../../application/view"
import { textContent, type AgentFeatures, type Conversation } from "../../model"
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
const agentFeatures: AgentFeatures = {
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
}

function wireView(
  conversationId: string,
  change: Partial<ConversationView>,
): ConversationView {
  return {
    conversationId,
    revision: "r1",
    title: null,
    messages: [],
    pending: [],
    permissions: [],
    questions: [],
    tools: [],
    queueComplete: true,
    transcriptState: "complete",
    truncated: false,
    approvalMode: "ask",
    approvalModes: [],
    capabilities: {
      queue: true,
      steer: true,
      resume: true,
      permissions: true,
      imageInput: false,
      agentFeatures,
    },
    lifecycle: { phase: "attached" },
    ...change,
  }
}

let container: HTMLDivElement
let root: Root

async function render(conversation: Conversation) {
  await React.act(async () => {
    root.render(
      <Provider store={store as never}>
        <Transcript
          conversation={conversation}
          ground="paper"
          animateMount={false}
          streamText={false}
          emptyState={false}
          statusLabel="Ready"
          gatewayAvailable
          onOpenPaste={vi.fn()}
        />
      </Provider>,
    )
  })
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

it("uses observed submission causality through beginSend, replacement views and real rendering", async () => {
  const runtime = {
    agent: "claude",
    provider: "anthropic",
    model: "unknown",
    modelName: "Unknown",
    workspace: "/tmp",
    contextWindowTokens: 200000,
    reasoning: true,
  }
  const refusal = wireView("c0", {
    runtime,
    messages: [
      {
        executionId: "B",
        userText: "Later wire input",
        attachments: [],
        files: [],
        parts: [],
        status: "failed",
        authenticationRequired: true,
      },
    ],
    pending: [],
  })
  let tabs = beginSend(emptyLocalTabs(), {
    conversationId: "c0",
    executionId: "A",
    actionId: "action-A",
    mode: "queued",
    content: textContent("Older local failure"),
  })
  tabs = failSend(tabs, "c0", "A", "Offline", { kind: "uncertain" })
  const older = tabs.conversations[0]
  if (!older) throw new Error("missing local conversation")
  let current = applyView(older, refusal)
  expect(current.turns.at(-1)).toMatchObject({ executionId: "A", observedInput: null })
  await render(current)
  expect(container.querySelector(".provider-sign-in")).not.toBeNull()
  expect(container.textContent).toContain("Older local failure")
  current = applyView(current, { ...refusal, revision: "new-revision-same-B" })
  await render(current)
  expect(container.querySelector(".provider-sign-in")).not.toBeNull()
  const replay = reattemptSend({ ...tabs, conversations: [current] }, "c0", "A")
  const replayed = replay.conversations[0]
  if (!replayed) throw new Error("missing replayed conversation")
  await render(applyView(replayed, refusal))
  expect(container.querySelector(".provider-sign-in")).toBeNull()
  let newer = beginSend(
    { ...tabs, conversations: [current] },
    {
      conversationId: "c0",
      executionId: "C",
      actionId: "action-C",
      mode: "queued",
      content: textContent("Newer local input"),
    },
  )
  const sending = newer.conversations[0]
  if (!sending) throw new Error("missing sending conversation")
  await render(sending)
  expect(container.querySelector(".provider-sign-in")).toBeNull()
  newer = failSend(newer, "c0", "C", "Offline", { kind: "uncertain" })
  const failed = newer.conversations[0]
  if (!failed) throw new Error("missing failed conversation")
  let replaced = applyView(failed, refusal)
  await render(replaced)
  expect(container.querySelector(".provider-sign-in")).toBeNull()
  expect(
    container.querySelector("[data-slot=transcript-divider]")?.textContent,
  ).toContain("failed")
  const pending = applyView(current, {
    ...refusal,
    pending: [
      {
        executionId: "C",
        text: "Accepted pending",
        attachments: [],
        files: [],
        mode: "queued",
      },
    ],
  })
  await render(pending)
  expect(container.querySelector(".provider-sign-in")).toBeNull()
  replaced = applyView(replaced, {
    ...refusal,
    messages: [
      ...refusal.messages,
      {
        executionId: "C",
        userText: "Newer local input",
        attachments: [],
        files: [],
        parts: [],
        status: "running",
      },
    ],
  })
  expect(replaced.turns.filter((turn) => turn.executionId === "C")).toHaveLength(1)
  await render(replaced)
  expect(container.querySelector(".provider-sign-in")).toBeNull()
})
