/**
 * A conversation that opened with one record it could not read, on both
 * surfaces at once: the desktop window over a fake gateway, and the panel's
 * transcript beside it. Two turns of text stay visible. One line sits before
 * them and one after the turn it followed. The gap list was cut, so both
 * surfaces end with "More parts of this conversation couldn't be read".
 * `transcript-unreadable.mjs` drives it.
 */
import type {
  AgentsApi,
  AttachmentApi,
  AuthApi,
  GithubApi,
  VoiceApi,
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
  view,
} from "../../../../src/desktop/workspace/adapters/gateway/fake-gateway"
import { saveFile } from "../../../../src/host"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "../../../../src/desktop/styles.css"

const conversation = "7d1e2f3a-4b5c-4d6e-8f0a-1b2c3d4e5f61"
const sentence = "Couldn't read this part of the conversation"
const more = "More parts of this conversation couldn't be read"

const gateway = fakeGateway()
gateway.rows.set(
  conversation,
  row(conversation, { title: "Unreadable record", preview: "Here is the plan." }),
)
const wireView = conversationView(
  view(conversation, {
    revision: "1",
    messages: [
      {
        executionId: "run-1",
        userText: "Plan the release.",
        attachments: [],
        files: [],
        status: "completed",
        parts: [
          {
            offset: 0,
            kind: "text",
            text: "Here is the plan.",
            toolId: "",
            noticeId: "",
          },
        ],
      },
    ],
    unreadable: [{ id: "3" }, { id: "9", afterExecutionId: "run-1" }],
    unreadableTruncated: true,
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
        github: {} as GithubApi,
        voice: {} as VoiceApi,
        vault: {} as import("@nessa/client").VaultApi,
        // The Devices page is not opened here.
        devices: {} as import("@nessa/client").DevicesApi,
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
            emptyState
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
  __unreadable: {
    sentence,
    more,
    order: ["3", "Plan the release.", "Here is the plan.", "9"],
  },
})
