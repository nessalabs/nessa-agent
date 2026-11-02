/**
 * The files an agent published (slice D2b), on both surfaces at once: the
 * desktop window over a fake gateway (`fake-gateway.ts`), and two panel
 * transcripts beside it — one conversation with a turn, one with no turn and
 * only a file published while none ran. `published-files.mjs` drives it.
 *
 * Every surface reads through the real client (`createAttachmentApi`'s
 * `fetch`, ranges and SHA-256 and all) and its own `HeldFilesProvider`, over
 * a fake `artifact.read` that serves the bytes below a range at a time,
 * slowly enough for reads to overlap. Like the gateway, it serves four reads
 * at once across every surface and answers the rest busy; it counts, per
 * surface, the most it was asked at once (`__published.stats`).
 *
 * The files: a picture of three ranges (`noise.png`, 160 × 120), a text file,
 * a file of 2.5 MiB that a host save sends in several parts (`large.bin`),
 * ten small pictures, a picture the conversation no longer keeps
 * (`gone.png`), and one it cannot read until the check says so
 * (`flaky.png`, `__published.allowFlaky`). Saving goes through the host's own
 * port (`saveFile`): a browser download here, and the desktop app's
 * `save_file_*` commands when the check stands in for the host
 * (`window.__TAURI_INTERNALS__`, set before the page loads).
 */
import {
  NessaRpcError,
  ProductMethod,
  type AgentsApi,
  type AuthApi,
  type GithubApi,
  type VoiceApi,
  type ConversationView,
  type CredentialApi,
  type McpAppsApi,
  type McpServersApi,
  type PairingApi,
} from "@nessa/client"
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { createAttachmentApi } from "../../../../packages/nessa-client/src/presentation/attachment-api"
import { conversationView } from "../../../../packages/nessa-client/src/protocol/conversation-validate"
import {
  HeldFilesProvider,
  Transcript,
  heldFileReader,
  type HeldFileReader,
} from "../../../../src/conversation"
import {
  ClockProvider,
  ClockProvider as ConversationClockProvider,
} from "../../../../src/clock"
import { textContent, type Conversation } from "../../../../src/conversation/model"
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

type ConversationArtifact = ConversationView["artifacts"][number]

const conversation = "0b9a3c1e-5d2f-4a7b-8c6d-1e2f3a4b5c6d"
const looseConversation = "5c6d1e2f-3a4b-4c6d-8e2f-0b9a3c1e5d2f"
const execution = "run"

/** A picture of noise, the same every load: it compresses to nearly its raw size. */
async function noisyPng(width: number, height: number, seed: number) {
  const canvas = document.createElement("canvas")
  canvas.width = width
  canvas.height = height
  const context = canvas.getContext("2d")
  if (!context) throw new Error("no 2d canvas")
  const image = context.createImageData(width, height)
  let state = seed >>> 0 || 1
  for (let index = 0; index < image.data.length; index++) {
    state ^= state << 13
    state ^= state >>> 17
    state ^= state << 5
    image.data[index] = index % 4 === 3 ? 255 : (state >>> 0) & 255
  }
  context.putImageData(image, 0, 0)
  const blob = await new Promise<Blob | null>((resolve) =>
    canvas.toBlob(resolve, "image/png"),
  )
  if (!blob) throw new Error("the canvas made no picture")
  return new Uint8Array(await blob.arrayBuffer())
}

async function digestOf(bytes: Uint8Array) {
  const hash = new Uint8Array(
    await crypto.subtle.digest("SHA-256", bytes as Uint8Array<ArrayBuffer>),
  )
  return `sha256:${[...hash].map((byte) => byte.toString(16).padStart(2, "0")).join("")}`
}

function base64(bytes: Uint8Array) {
  let text = ""
  for (const byte of bytes) text += String.fromCharCode(byte)
  return btoa(text)
}

type Published = { name: string; mimeType: string; bytes?: Uint8Array; size?: number }

const published: Published[] = [
  { name: "noise.png", mimeType: "image/png", bytes: await noisyPng(160, 120, 7) },
  {
    name: "notes.txt",
    mimeType: "text/plain",
    bytes: new TextEncoder().encode("Published notes.\n"),
  },
  {
    name: "large.bin",
    mimeType: "application/octet-stream",
    // Past two 1 MiB parts, and not a multiple of one.
    bytes: Uint8Array.from(
      { length: 2.5 * 1024 * 1024 + 3 },
      (_, index) => (index * 31 + 7) & 0xff,
    ),
  },
  ...(await Promise.all(
    Array.from({ length: 10 }, async (_, index) => ({
      name: `tile-${index}.png`,
      mimeType: "image/png",
      bytes: await noisyPng(40 + index, 30, 100 + index),
    })),
  )),
  { name: "flaky.png", mimeType: "image/png", bytes: await noisyPng(48, 36, 3) },
  // Recorded, and no longer kept: its bytes are nowhere.
  { name: "gone.png", mimeType: "image/png", size: 10 },
]
const loose: Published = {
  name: "loose.txt",
  mimeType: "text/plain",
  bytes: new TextEncoder().encode("Published while no turn ran.\n"),
}

const held = new Map<string, Uint8Array>()
async function artifactOf(file: Published, executionId?: string) {
  const digest = file.bytes ? await digestOf(file.bytes) : `sha256:${"e".repeat(64)}`
  if (file.bytes) held.set(digest, file.bytes)
  const artifact: ConversationArtifact = {
    lease: "lease",
    name: file.name,
    digest,
    mimeType: file.mimeType,
    size: file.bytes?.byteLength ?? file.size ?? 1,
    ...(executionId === undefined ? {} : { executionId }),
  }
  return artifact
}
const artifacts = await Promise.all(published.map((file) => artifactOf(file, execution)))
const looseArtifact = await artifactOf(loose)
const flakyDigest = artifacts.find((artifact) => artifact.name === "flaky.png")?.digest

// The gateway's read slots, shared by every surface, and what each surface asked.
const GATEWAY_READS_AT_ONCE = 4
let reading = 0
let busy = 0
let flakyAllowed = false
const peaks: Record<string, number> = {}
const asked: Record<string, number> = {}

/** One surface's client: its own `artifact.read`, counted, over the shared slots. */
function attachmentsFor(surface: string) {
  let going = 0
  peaks[surface] = 0
  asked[surface] = 0
  const session = {
    async request(method: string, params: unknown) {
      if (method !== ProductMethod.ArtifactRead) throw new Error(`unexpected ${method}`)
      const range = params as {
        digest: string
        offset: number
        maxBytes: number
        size: number
      }
      if (reading >= GATEWAY_READS_AT_ONCE) {
        busy++
        throw new NessaRpcError("temporarily_unavailable", "every read slot is taken")
      }
      reading++
      going++
      asked[surface] = (asked[surface] ?? 0) + 1
      peaks[surface] = Math.max(peaks[surface] ?? 0, going)
      try {
        // Slow enough that a transcript's reads overlap.
        await new Promise((resolve) => setTimeout(resolve, 30))
        if (range.digest === flakyDigest && !flakyAllowed)
          throw new NessaRpcError("attachment_unavailable", "could not read it now")
        const bytes = held.get(range.digest)
        if (!bytes) throw new NessaRpcError("attachment_not_found", "not held")
        const end = Math.min(range.offset + range.maxBytes, range.size)
        return { offset: range.offset, data: base64(bytes.subarray(range.offset, end)) }
      } finally {
        reading--
        going--
      }
    },
  }
  return createAttachmentApi(
    session as never,
    { put: () => Promise.reject(new Error("this fixture uploads nothing")) },
    () => crypto.randomUUID(),
    (ms, elapsed) => {
      const id = window.setTimeout(elapsed, ms)
      return () => window.clearTimeout(id)
    },
  )
}

// The desktop window, over the fake gateway.
const gateway = fakeGateway()
gateway.rows.set(
  conversation,
  row(conversation, { title: "Published files", preview: "Here they are." }),
)
const desktopView: ConversationView = conversationView(
  view(conversation, {
    revision: "1",
    messages: [
      {
        executionId: execution,
        userText: "Make me some files.",
        attachments: [],
        files: [],
        status: "completed",
        parts: [
          { offset: 0, kind: "text", text: "Here they are.", toolId: "", noticeId: "" },
        ],
      },
    ],
    artifacts,
  }),
  conversation,
)
gateway.views.set(conversation, desktopView)
const desktopAttachments = attachmentsFor("desktop")
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
        attachments: desktopAttachments,
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

// The panel's transcript: a conversation with its turn, and one with none.
function panelConversation(
  id: string,
  serverId: string,
  files: ConversationArtifact[],
  withTurn: boolean,
): Conversation {
  return {
    id,
    serverConversationId: serverId,
    title: id,
    phase: "idle",
    draft: [],
    turns: withTurn
      ? [
          {
            id: `${execution}:user`,
            from: "user",
            executionId: execution,
            receipt: "delivered",
            content: textContent("Make me some files."),
          },
          {
            id: `${execution}:assistant`,
            from: "assistant",
            executionId: execution,
            text: "Here they are.",
            status: "completed",
            parts: [
              {
                offset: 0,
                kind: "text",
                text: "Here they are.",
                toolId: "",
                noticeId: "",
              },
            ],
          },
        ]
      : [],
    remote: {
      artifacts: files.map((file) => ({
        name: file.name,
        digest: file.digest,
        mimeType: file.mimeType,
        size: file.size,
        ...(file.executionId === undefined ? {} : { executionId: file.executionId }),
      })),
      notes: [],
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
        agentFeatures: desktopAgentFeatures,
      },
      lifecycle: { phase: "attached" },
      queueComplete: true,
      transcriptState: "complete",
      truncated: false,
    },
  } as Conversation
}
const desktopAgentFeatures = {
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
} as const

const panelStore = {
  dispatch: () => {},
  getState: () => ({}),
  subscribe: () => () => {},
  replaceReducer: () => {},
}
// One reader per surface, made once: a different reader is a different provider.
const panelAttachments = attachmentsFor("panel")
const panelReader: HeldFileReader = heldFileReader(() => panelAttachments)
const looseAttachments = attachmentsFor("panel-loose")
const looseReader: HeldFileReader = heldFileReader(() => looseAttachments)

function renderPanel(
  elementId: string,
  value: Conversation,
  read: HeldFileReader,
  emptyState: boolean,
) {
  const element = document.getElementById(elementId)
  if (!element) throw new Error(`missing #${elementId}`)
  createRoot(element).render(
    <React.StrictMode>
      <Provider store={panelStore as never}>
        <ConversationClockProvider now={dependencies.now}>
          <HeldFilesProvider read={read} save={saveFile}>
            <Transcript
              conversation={value}
              ground="paper"
              animateMount={false}
              streamText={false}
              emptyState={emptyState}
              statusLabel="Nothing here yet"
              gatewayAvailable
              onOpenPaste={() => {}}
            />
          </HeldFilesProvider>
        </ConversationClockProvider>
      </Provider>
    </React.StrictMode>,
  )
}
renderPanel(
  "panel",
  panelConversation("panel", conversation, artifacts, true),
  panelReader,
  false,
)
renderPanel(
  "panel-loose",
  panelConversation("panel-loose", looseConversation, [looseArtifact], false),
  looseReader,
  true,
)

Object.assign(window, {
  __published: {
    /** The most reads each surface had at once, what it asked, and the busy answers. */
    stats: () => ({ peaks: { ...peaks }, asked: { ...asked }, busy }),
    /** A published file's bytes, base64, to hold a save to. */
    bytesOf: (name: string) => {
      const file = [...published, loose].find((each) => each.name === name)
      return file?.bytes ? base64(file.bytes) : null
    },
    /** `flaky.png` can be read from now on. */
    allowFlaky: () => {
      flakyAllowed = true
    },
    picture: { name: "noise.png", width: 160, height: 120 },
  },
})
