import { expect, it, vi } from "vitest"
import {
  createSubscriptionGate,
  type ConversationView,
  type NessaClient,
  type ViewSubscriptionHandlers,
} from "@nessa/client"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import { scenarioEffects } from "../scenario/effects"
import { gatewayEffects } from "../gateway/effects"
import { bindConversation, controlConversation, followConversation } from "./slice"

// The rows are the panel rows of `docs/design/record-subscriptions.md`.

function gatewayView(): ConversationView {
  return {
    questions: [],
    artifacts: [],
    conversationId: "server",
    title: null,
    revision: "capabilities",
    approvalMode: "ask",
    approvalModes: [
      {
        id: "ask",
        name: "Provider asks",
        description: "The provider asks where required.",
      },
    ],
    messages: [],
    pending: [],
    permissions: [],
    tools: [],
    capabilities: {
      queue: true,
      steer: true,
      resume: true,
      permissions: true,
      imageInput: false,
      agentFeatures: {
        permissionDenial: "supported_for_offered_permission_reviews",
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
    truncated: false,
    queueComplete: true,
    transcriptState: "complete",
  }
}

it("P8: a tab followed while its after-command single read is opening still settles the control", async () => {
  const opens: Array<{ handlers: ViewSubscriptionHandlers; answer: () => void }> = []
  const gate = createSubscriptionGate()
  const client = {
    connectionState: { status: "connected" },
    subscriptions: {
      view: (
        id: string,
        handlers: ViewSubscriptionHandlers,
        options: { signal?: AbortSignal } = {},
      ) =>
        gate(`view:${id}`, options.signal, async () => {
          await new Promise<void>((answer) => opens.push({ handlers, answer }))
          return { id: `s${opens.length}`, close: vi.fn(async () => {}) }
        }),
    },
  } as unknown as NessaClient
  const effects = scenarioEffects("echo")
  const gateway = gatewayEffects(
    () => client,
    () => new Promise(() => {}),
  )
  const store = makeStore(
    createDependencies({
      conversation: {
        ...effects,
        follow: gateway.follow,
        reorder: vi.fn(async () => "applied" as const),
      },
    }),
  )
  store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
  let settled = false
  void store
    .dispatch(
      controlConversation({ id: "c0", control: { kind: "reorder", executionIds: [] } }),
    )
    .finally(() => (settled = true))
  // The control is answered and its result read once: the tab is not on screen.
  await vi.waitFor(() => expect(opens).toHaveLength(1))
  // Switched to before that read's first frame.
  void store.dispatch(followConversation("c0"))
  opens[0]!.answer()
  await vi.waitFor(() => expect(opens).toHaveLength(2))
  opens[1]!.answer()
  await new Promise((done) => setTimeout(done, 0))
  opens[1]!.handlers.view({
    cursor: { incarnation: "i", position: "1" },
    view: gatewayView(),
  })
  await vi.waitFor(() => expect(settled).toBe(true))
  expect(store.getState().conversation.conversations[0]!.controlPending).toBe(false)
})
