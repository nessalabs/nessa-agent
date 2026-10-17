/**
 * Notes left in a conversation, on both surfaces at once: the desktop window
 * over a fake gateway (`fake-gateway.ts`), and the panel's transcript beside
 * it, both reading the same view — two turns, two inputs still waiting, and
 * the same notes: one in each state a note can be in, from the owner and from
 * someone it is shared with, one left before any turn, one after a turn the
 * bounded view left out, one left between the two inputs waiting, and one
 * whose text is a single word far wider than either column.
 * `transcript-notes.mjs` drives it.
 *
 * The view is the gateway's own shape: every input waiting is listed both as
 * a queued turn and as pending. The desktop window reads it through the real
 * client's check (`conversationView`) and its own adapter (`transcriptFrom`);
 * the panel applies it as its follow does (`applyView`), each note read the
 * one way both read it (`shownNote`).
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
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { conversationView } from "../../../../packages/nessa-client/src/protocol/conversation-validate"
import {
  HeldFilesProvider,
  Transcript,
  shownNote,
  type HeldFileReader,
} from "../../../../src/conversation"
import {
  ClockProvider,
  ClockProvider as ConversationClockProvider,
} from "../../../../src/clock"
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
  note,
  row,
  view,
} from "../../../../src/desktop/workspace/adapters/gateway/fake-gateway"
import { saveFile } from "../../../../src/host"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "../../../../src/desktop/styles.css"

const conversation = "7d1e2f3a-4b5c-4d6e-8f0a-1b2c3d4e5f60"
const turns = [
  { executionId: "run-1", asked: "Plan the release.", said: "Here is the plan." },
  { executionId: "run-2", asked: "Ship it.", said: "Shipped." },
]
const owner = { kind: "owner" as const, surface: "desktop" }
const shared = { kind: "grant" as const, surface: "phone" }
/** One word far wider than any column: it has to wrap inside its row. */
const longWord = `deploy-window-${"x".repeat(320)}`

/** Inputs still waiting to run, in the order they wait, after both turns. */
const waiting = [
  { executionId: "run-3", text: "Tag the release." },
  { executionId: "run-4", text: "Announce it." },
]

const notes: ConversationView["notes"] = [
  note("before", { text: "Left before anything was asked.", author: owner }),
  note("dropped", {
    text: "Written after a turn this view no longer holds.",
    author: owner,
    // Older than every turn the view keeps: shown before the first.
    afterExecutionId: "run-0",
  }),
  note("assigned", {
    text: "Use the staging database for this.",
    author: owner,
    delivery: "next_turn",
    deliveryState: "assigned",
    executionId: "run-2",
    afterExecutionId: "run-1",
  }),
  note("expired", {
    text: "Remember the freeze on Friday.",
    author: shared,
    delivery: "next_turn",
    deliveryState: "retired",
    retiredReason: "expired",
    afterExecutionId: "run-1",
  }),
  note("pending", {
    text: "Mention the changelog next time.",
    author: shared,
    delivery: "next_turn",
    deliveryState: "pending",
    afterExecutionId: "run-2",
  }),
  note("long", { text: longWord, author: shared, afterExecutionId: "run-2" }),
  note("lost", {
    text: "Loop in the on-call engineer.",
    author: shared,
    delivery: "next_turn",
    deliveryState: "retired",
    retiredReason: "permission_lost",
    // Left between the two inputs waiting: right after the first.
    afterExecutionId: waiting[0]!.executionId,
  }),
]

// The desktop window, over the fake gateway.
const gateway = fakeGateway()
gateway.rows.set(
  conversation,
  row(conversation, { title: "Transcript notes", preview: "Shipped." }),
)
const wireView = conversationView(
  view(conversation, {
    revision: "1",
    messages: [
      ...turns.map((turn) => ({
        executionId: turn.executionId,
        userText: turn.asked,
        attachments: [],
        files: [],
        status: "completed" as const,
        parts: [
          { offset: 0, kind: "text" as const, text: turn.said, toolId: "", noticeId: "" },
        ],
      })),
      ...waiting.map((input) => ({
        executionId: input.executionId,
        userText: input.text,
        attachments: [],
        files: [],
        status: "queued" as const,
        parts: [],
      })),
    ],
    pending: waiting.map((input) => ({
      ...input,
      attachments: [],
      files: [],
      mode: "queued" as const,
    })),
    // Bounded: it left out run-0, older than every turn it keeps.
    truncated: true,
    notes,
  }),
  conversation,
)
gateway.views.set(conversation, wireView)
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

// The panel's transcript of the same view, applied as its follow applies it.
const panelConversation = applyView(newConversation("panel"), {
  ...wireView,
  reviewsWaiting: [],
  questionsWaiting: [],
  questions: [],
  permissions: [],
  notes: wireView.notes.map(shownNote),
})
const panelStore = {
  dispatch: () => {},
  getState: () => ({}),
  subscribe: () => () => {},
  replaceReducer: () => {},
}
const readNothing: HeldFileReader = () => Promise.reject(new Error("no files here"))
const panelElement = document.getElementById("panel")
if (!panelElement) throw new Error("missing #panel")
createRoot(panelElement).render(
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
  </React.StrictMode>,
)

Object.assign(window, {
  __notes: {
    /** Each note's id, in the order the transcripts must show it among the turns. */
    order: [
      "before",
      "dropped",
      "Plan the release.",
      "Here is the plan.",
      "assigned",
      "expired",
      "Ship it.",
      "Shipped.",
      "pending",
      "long",
      "Tag the release.",
      "lost",
      "Announce it.",
    ],
  },
})
