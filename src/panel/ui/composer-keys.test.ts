// @vitest-environment jsdom
/**
 * The #107 regression at the surface where it was visible: the real panel,
 * its real queue, and the real design-system composer.
 *
 * The queue and composer once shared the active conversation's key. With the
 * absent notices that preceded them at the time, React retained old queue
 * subtrees across renders and conversation switches. A local imitation of the
 * children would keep passing if the production keys regressed, so this mounts
 * App and drives its actual store projection.
 */
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

import {
  bindConversation,
  openConversation,
  readStarted,
  scenarioEffects,
  setActive,
  viewReceived,
  type ConversationView,
} from "../../conversation/testing"
import { makeStore, type AppStore } from "../../store"
import { sessionReady } from "../../session/adapters/store/slice"
import { setSelection } from "../../conversation/adapters/store/slice"
import { createAttachmentResources } from "../adapters/attachment-resources"
import { App } from "./app"
import type { ConversationChoices } from "./use-agent-choices"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

let host: HTMLDivElement
let root: Root
let store: AppStore
let attachmentResources: ReturnType<typeof createAttachmentResources>
let animateDescriptor: PropertyDescriptor | undefined

function view(
  conversationId: string,
  queued: number,
  revision: string,
): ConversationView {
  return {
    questions: [],
    conversationId,
    approvalMode: "ask",
    approvalModes: [
      { id: "ask", name: "Provider asks", description: "Provider asks where required." },
    ],
    title: null,
    revision,
    queueComplete: true,
    truncated: false,
    messages: [],
    pending: Array.from({ length: queued }, (_, index) => ({
      executionId: `${conversationId}:queued:${index}`,
      text: `Waiting ${index + 1}`,
      attachments: [],
      files: [],
      mode: "queued" as const,
    })),
    permissions: [],
    tools: [],
    capabilities: {
      queue: true,
      steer: false,
      resume: false,
      permissions: false,
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
  }
}

function queue(conversationId: string, count: number, revision: string) {
  const requestId = `${conversationId}:read`
  store.dispatch(bindConversation({ id: conversationId, serverId: conversationId }))
  store.dispatch(readStarted({ id: conversationId, requestId }))
  store.dispatch(
    viewReceived({
      id: conversationId,
      requestId,
      serverId: conversationId,
      view: view(conversationId, count, revision),
    }),
  )
}

function panel(choices?: ConversationChoices) {
  return React.createElement(
    React.StrictMode,
    null,
    React.createElement(Provider, {
      store,
      children: React.createElement(App, {
        attachmentResources,
        canChoosePaths: false,
        digest: async () => "digest",
        loadConversationChoices: async () => ({
          catalog: choices?.catalog ?? { agents: [] },
          chosenAgent: choices?.chosenAgent,
        }),
      }),
    }),
  )
}

it("shows the catalog model beside the voice control on a new draft", async () => {
  store.dispatch(
    sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
  )
  const choices: ConversationChoices = {
    chosenAgent: "claude",
    catalog: {
      agents: [
        {
          agent: "claude",
          defaultModel: "claude-sonnet-5",
          models: [
            {
              modelId: "claude-sonnet-5",
              displayName: "Sonnet 5",
              maxContextWindowTokens: 1_000_000,
              reasoning: true,
              imageInput: true,
              approvalModes: [
                {
                  id: "ask",
                  name: "Provider asks",
                  description: "Claude asks where required.",
                },
              ],
            },
          ],
        },
      ],
    },
  }
  await act(async () => root.render(panel(choices)))
  const picker = host.querySelector<HTMLButtonElement>(
    '[data-slot="model-picker-trigger"]',
  )
  expect(picker?.getAttribute("aria-label")).toBe("Model: Sonnet 5")
  expect(picker?.classList.contains("nessa-composer-model")).toBe(true)
  expect(picker?.parentElement?.nextElementSibling?.getAttribute("aria-label")).toBe(
    "Start voice input",
  )

  act(() => {
    store.dispatch(
      setSelection({
        id: "c0",
        selection: {
          agent: "claude",
          model: "claude-retired-4",
          approvalMode: "ask",
        },
      }),
    )
    store.dispatch(bindConversation({ id: "c0", serverId: "saved" }))
  })
  await act(async () => root.render(panel(choices)))
  expect(
    host
      .querySelector<HTMLButtonElement>('[data-slot="model-picker-trigger"]')
      ?.getAttribute("aria-label"),
  ).toBe("Model: claude-retired-4")
})

function chips() {
  return host.querySelectorAll('[data-slot="composer-queue-badge"]')
}

beforeEach(() => {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  }))
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
  animateDescriptor = Object.getOwnPropertyDescriptor(Element.prototype, "animate")
  Object.defineProperty(Element.prototype, "animate", {
    configurable: true,
    value: vi.fn(() => ({ cancel: vi.fn() })),
  })
  attachmentResources = createAttachmentResources()
  store = makeStore({
    attachments: attachmentResources,
    canChoosePaths: false,
    conversation: scenarioEffects("echo"),
  })
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
  if (animateDescriptor)
    Object.defineProperty(Element.prototype, "animate", animateDescriptor)
  else delete (Element.prototype as { animate?: unknown }).animate
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

// The eight renders are the regression itself: #107 leaked one retained
// subtree per render, so fewer renders would test less. Mounting the real
// panel that many times takes 0.6–3 s, and up to 6.3 s in a loaded full run,
// past vitest's 5 s default; this test's own timeout covers that.
it("keeps one queue chip in its conversation across repeated renders and tab switches", async () => {
  queue("c0", 2, "r1")

  for (let render = 0; render < 8; render += 1) {
    await act(async () => root.render(panel()))
  }
  expect(chips()).toHaveLength(1)
  expect(chips()[0]?.textContent).toBe("Queued 2")

  await act(async () => queue("c0", 3, "r2"))
  expect(chips()).toHaveLength(1)
  expect(chips()[0]?.textContent).toBe("Queued 3")

  await act(async () => queue("c0", 0, "r3"))
  expect(chips()).toHaveLength(0)

  await act(async () => queue("c0", 2, "r4"))
  expect(chips()).toHaveLength(1)
  expect(chips()[0]?.textContent).toBe("Queued 2")

  await act(async () => {
    store.dispatch(openConversation())
  })
  expect(chips()).toHaveLength(0)

  await act(async () => {
    store.dispatch(setActive("c0"))
  })
  expect(chips()).toHaveLength(1)
  expect(chips()[0]?.textContent).toBe("Queued 2")
}, 20_000)
