/**
 * Turn endings and a question, on both surfaces at once: the desktop window
 * over a fake gateway, and the panel's transcript of the same view.
 * `turn-outcomes.mjs` drives it.
 *
 * The view is the gateway's own shape. One turn failed before it said
 * anything, one was cancelled after a partial reply, one was left unresolved
 * with no diagnostic, and one is still running and asking. A notice says a
 * review was left out. The desktop reads the view through the real client's
 * check and `transcriptFrom`. The panel applies it as its follow does.
 * Answering on the desktop asks the gateway; the next frame is published and
 * the panel is applied again. `forget` publishes a frame that no longer lists
 * a question, which is what an answer on either surface becomes.
 */
import type {
  AgentsApi,
  AttachmentApi,
  AuthApi,
  ConversationView,
  CredentialApi,
  McpAppsApi,
  McpServersApi,
  PairingApi,
} from "@nessa/client"
import * as React from "react"
import { flushSync } from "react-dom"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { conversationView } from "../../../../packages/nessa-client/src/protocol/conversation-validate"
import {
  ClockProvider,
  ClockProvider as ConversationClockProvider,
} from "../../../../src/clock"
import {
  HeldFilesProvider,
  Transcript,
  shownNote,
  type HeldFileReader,
} from "../../../../src/conversation"
import { applyView } from "../../../../src/conversation/application/usecases/apply-view"
import { conversation as newConversation } from "../../../../src/conversation/model"
import { createDesktopDependencies } from "../../../../src/desktop/dependencies"
import { makeDesktopStore } from "../../../../src/desktop/store"
import { DesktopIconFamilyProvider } from "../../../../src/desktop/ui/icons"
import { DesktopWindow } from "../../../../src/desktop/ui/desktop-window"
import { WidgetRegistryProvider } from "../../../../src/desktop/widgets"
import { followWorkspace, loadWorkspace } from "../../../../src/desktop/workspace"
import {
  fakeGateway,
  row,
  shownQuestion,
  view,
} from "../../../../src/desktop/workspace/adapters/gateway/fake-gateway"
import { saveFile } from "../../../../src/host"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "../../../../src/desktop/styles.css"

const conversation = "7d1e2f3a-4b5c-4d6e-8f0a-1b2c3d4e5f61"

const field = (key: string, prompt: string, value: string, label: string) => ({
  key,
  prompt,
  multiSelect: false,
  freeText: false,
  required: true,
  options: [{ value, label }],
})

const wire = view(conversation, {
  revision: "1",
  runtime: {
    model: "claude-opus",
    provider: "anthropic",
    workspace: "/",
    agent: "claude",
    modelName: "Claude",
    contextWindowTokens: 1,
    reasoning: false,
  },
  messages: [
    {
      executionId: "run-fail",
      userText: "Check the deploy.",
      attachments: [],
      files: [],
      status: "failed",
      error: "Provider connection lost",
      parts: [],
    },
    {
      executionId: "run-cancel",
      userText: "Keep going.",
      attachments: [],
      files: [],
      status: "cancelled",
      error: "Stopped",
      parts: [
        {
          offset: 0,
          kind: "text",
          text: "Halfway",
          toolId: "",
          noticeId: "",
        },
      ],
    },
    {
      executionId: "run-open",
      userText: "Wait.",
      attachments: [],
      files: [],
      status: "unresolved",
      parts: [],
    },
    {
      executionId: "run-ask",
      userText: "Ask me where.",
      attachments: [],
      files: [],
      status: "running",
      parts: [],
    },
  ],
  questions: [
    {
      executionId: "run-ask",
      questionId: "place",
      message: "Where should this land?",
      questions: [field("where", "Pick a place", "staging", "Staging")],
    },
    {
      executionId: "run-ask",
      questionId: "when",
      message: "When should it ship?",
      questions: [field("when", "Pick a time", "today", "Today")],
    },
    {
      executionId: "run-ask",
      questionId: "elsewhere",
      message: "Who should hear?",
      questions: [field("who", "Pick someone", "ops", "Ops")],
    },
  ],
  interactionViewError: "A review was left out of this view.",
})

const gateway = fakeGateway()
gateway.rows.set(
  conversation,
  row(conversation, { title: "Turn outcomes", preview: "Ask me where.", running: true }),
)
const wireView = conversationView(wire, conversation)
gateway.views.set(conversation, wireView)

const original = gateway.client.conversation.answerQuestion.bind(
  gateway.client.conversation,
)
gateway.client.conversation.answerQuestion = (async (
  ...args: Parameters<typeof original>
) => {
  const result = await original(...args)
  gateway.publish(conversation)
  showPanel()
  return result
}) as typeof original

const dependencies = createDesktopDependencies({
  gateway: () =>
    Promise.resolve(
      Object.assign(gateway.client, {
        mcpApps: {} as McpAppsApi,
        mcpServers: {} as McpServersApi,
        pairing: {} as PairingApi,
        credentials: {} as CredentialApi,
        auth: {} as AuthApi,
        attachments: {} as AttachmentApi,
        agents: {} as AgentsApi,
      }),
    ),
  apps: { sandbox: undefined, platform: "web" },
})
const store = makeDesktopStore(dependencies)
store.dispatch(followWorkspace())
void store.dispatch(loadWorkspace())

const rootElement = document.getElementById("root")
if (!rootElement) throw new Error("missing #root")
createRoot(rootElement).render(
  <React.StrictMode>
    <Provider store={store}>
      <HeldFilesProvider read={dependencies.heldFiles} save={saveFile}>
        <WidgetRegistryProvider registry={dependencies.widgets}>
          <ClockProvider now={dependencies.now}>
            <DesktopIconFamilyProvider>
              <DesktopWindow hostKind="browser" browserSurface inspectable={false} />
            </DesktopIconFamilyProvider>
          </ClockProvider>
        </WidgetRegistryProvider>
      </HeldFilesProvider>
    </Provider>
  </React.StrictMode>,
)

const panelStore = {
  dispatch: () => {},
  getState: () => ({}),
  subscribe: () => () => {},
  replaceReducer: () => {},
}
const readNothing: HeldFileReader = () => Promise.reject(new Error("no files here"))
const panelElement = document.getElementById("panel")
if (!panelElement) throw new Error("missing #panel")
let panelRoot: Root | undefined

function showPanel() {
  const current = gateway.views.get(conversation)
  if (!current || !panelElement) return
  const panelConversation = applyView(
    { ...newConversation(conversation), serverConversationId: conversation },
    {
      ...current,
      reviewsWaiting: [],
      permissions: [],
      questionsWaiting: [],
      questions: current.questions.flatMap((entry) => {
        const asked = shownQuestion(entry.question)
        return asked ? [asked] : []
      }),
      notes: current.notes.map(shownNote),
    },
  )
  const tree = (
    <React.StrictMode>
      <Provider store={panelStore as never}>
        <ConversationClockProvider now={dependencies.now}>
          <HeldFilesProvider read={readNothing} save={saveFile}>
            <Transcript
              conversation={panelConversation}
              ground="paper"
              animateMount={false}
              streamText={false}
              emptyState={false}
              statusLabel="Nothing here yet"
              gatewayAvailable
              onOpenPaste={() => {}}
            />
          </HeldFilesProvider>
        </ConversationClockProvider>
      </Provider>
    </React.StrictMode>
  )
  if (!panelRoot) panelRoot = createRoot(panelElement)
  // The page calls this from outside React. Flush so a frame that dropped a
  // question is on the panel before the caller looks.
  flushSync(() => panelRoot?.render(tree))
}

showPanel()

Object.assign(window, {
  __outcomes: {
    notice: "A review was left out of this view.",
    /** The next frame, as an answer on either surface leaves it. */
    forget(questionId: string) {
      const current = gateway.views.get(conversation)
      if (!current) return
      gateway.views.set(conversation, {
        ...current,
        questions: current.questions.filter(
          (entry) => shownQuestion(entry.question)?.questionId !== questionId,
        ),
      } satisfies ConversationView)
      gateway.publish(conversation)
      showPanel()
    },
  },
})
