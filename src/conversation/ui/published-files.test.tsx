// @vitest-environment jsdom
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import {
  HeldFileReadError,
  type HeldFileReader,
  type HeldFileSaver,
} from "../application/ports"
import {
  textContent,
  type AgentFeatures,
  type Conversation,
  type PublishedFile,
  type Turn,
} from "../model"
import { ClockProvider } from "../../clock"
import { HeldFilesProvider } from "./held-files"
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

const agentFeatures: AgentFeatures = {
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
}

const digest = (letter: string) => `sha256:${letter.repeat(64)}`
const report: PublishedFile = {
  executionId: "one",
  name: "report.pdf",
  digest: digest("a"),
  mimeType: "application/pdf",
  size: 2048,
}
const chart: PublishedFile = {
  executionId: "two",
  name: "chart.png",
  digest: digest("b"),
  mimeType: "image/png",
  size: 3,
}
/** Published while no turn ran. */
const loose: PublishedFile = {
  name: "notes.txt",
  digest: digest("c"),
  mimeType: "text/plain",
  size: 5,
}
/** Published by a turn the bounded view left out. */
const older: PublishedFile = { ...loose, executionId: "gone", name: "older.txt" }

function conversation(artifacts: PublishedFile[]): Conversation {
  const turn = (executionId: string, asked: string, said: string): Turn[] => [
    {
      id: `${executionId}:user`,
      from: "user" as const,
      executionId,
      receipt: "delivered" as const,
      content:
        executionId === "one"
          ? [
              ...textContent(asked),
              {
                type: "image-reference" as const,
                digest: digest("d"),
                mimeType: "image/webp" as const,
                size: 3,
              },
            ]
          : textContent(asked),
    },
    {
      id: `${executionId}:assistant`,
      from: "assistant" as const,
      executionId,
      text: said,
      status: "completed",
      parts: [{ offset: 0, kind: "text" as const, text: said, toolId: "", noticeId: "" }],
    },
  ]
  return {
    id: "tab",
    serverConversationId: "server",
    title: "Published",
    phase: "idle",
    draft: [],
    turns: [
      ...turn("one", "First ask", "First answer"),
      ...turn("two", "Second ask", "Second answer"),
    ],
    remote: {
      artifacts,
      notes: [],
      approvalMode: "ask",
      approvalModes: [{ id: "ask", name: "Provider asks", description: "Asks." }],
      questions: [],
      running: false,
      reviewsWaiting: [],
      permissions: [],
      tools: [],
      pending: [],
      capabilities: {
        queue: true,
        steer: true,
        resume: true,
        permissions: true,
        imageInput: true,
        agentFeatures,
      },
      lifecycle: { phase: "attached" },
      queueComplete: true,
      transcriptState: "complete",
      truncated: false,
    },
  }
}

let container: HTMLDivElement
let root: Root
const made: string[] = []
const revoked: string[] = []

async function render(
  value: Conversation,
  read: HeldFileReader,
  save: HeldFileSaver = async () => "saved",
  emptyState = false,
) {
  await React.act(async () => {
    root.render(
      <Provider store={store as never}>
        <HeldFilesProvider read={read} save={save}>
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
        </HeldFilesProvider>
      </Provider>,
    )
  })
}

async function settle() {
  await React.act(async () => new Promise((resolve) => setTimeout(resolve, 0)))
}

/** The text of the transcript in order, one entry per message or published file. */
function order() {
  return [
    ...container.querySelectorAll("[data-published-file], [data-slot='chat-bubble']"),
  ].map((element) =>
    element.hasAttribute("data-published-file")
      ? `file:${element.getAttribute("data-published-file")}`
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
  made.length = 0
  revoked.length = 0
  URL.createObjectURL = (() => {
    const url = `blob:test/${made.length + 1}`
    made.push(url)
    return url
  }) as typeof URL.createObjectURL
  URL.revokeObjectURL = ((url: string) => {
    revoked.push(url)
  }) as typeof URL.revokeObjectURL
  container = document.createElement("div")
  document.body.appendChild(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

it("shows each published file at the turn that published it, an older turn's first, and a loose one last", async () => {
  const read = vi.fn<HeldFileReader>(async () => new Blob(["png"]))
  await render(conversation([report, chart, loose, older]), read)
  const shown = order()
  expect(shown[0]).toBe("file:older.txt")
  expect(shown.indexOf("file:report.pdf")).toBeGreaterThan(shown.indexOf("First answer"))
  expect(shown.indexOf("file:report.pdf")).toBeLessThan(shown.indexOf("Second ask"))
  expect(shown.indexOf("file:chart.png")).toBeGreaterThan(shown.indexOf("Second answer"))
  expect(shown.at(-1)).toBe("file:notes.txt")
  // By name and size, whatever happens to its bytes.
  expect(container.textContent).toContain("report.pdf2.0 KB")
})

it("paints a published picture and a referenced image only from verified bytes, once", async () => {
  const pending = new Map<string, (bytes: Blob) => void>()
  const read = vi.fn<HeldFileReader>(
    (_conversation, file) => new Promise((resolve) => pending.set(file.digest, resolve)),
  )
  const value = conversation([report, chart])
  await render(value, read)
  // Only what is painted is read: the picture and the referenced image, not the PDF.
  expect(read.mock.calls.map(([id, file]) => [id, file.digest]).sort()).toEqual([
    ["server", chart.digest],
    ["server", digest("d")],
  ])
  // Until they arrive: stand-ins, labelled, and no picture.
  expect(container.querySelector("img")).toBeNull()
  expect(container.textContent).toContain("WebP image, 3 B")
  await React.act(async () => {
    pending.get(chart.digest)?.(new Blob(["png"]))
    pending.get(digest("d"))?.(new Blob(["webp"]))
  })
  await settle()
  const pictures = [...container.querySelectorAll("img")]
  expect(pictures.map((picture) => picture.getAttribute("alt")).sort()).toEqual([
    "WebP image, 3 B",
    "chart.png",
  ])
  // A newer view of the same conversation, the same files in it, reads nothing again.
  await render(conversation([report, chart]), read)
  await settle()
  expect(read).toHaveBeenCalledTimes(2)
  // Let go of when no longer shown.
  await React.act(async () => root.unmount())
  await settle()
  expect(revoked.sort()).toEqual([...made].sort())
  root = createRoot(container)
})

it("keeps the stand-in, and says why in the card and the tile alike, when the conversation no longer keeps a picture", async () => {
  const read = vi.fn<HeldFileReader>(async () => {
    throw new HeldFileReadError("not-held")
  })
  await render(conversation([chart]), read)
  await settle()
  expect(container.querySelector("img")).toBeNull()
  expect(
    [...container.querySelectorAll("[data-held-file='failed']")].map(
      (element) => element.textContent,
    ),
  ).toEqual(["No longer kept", "No longer kept"])
  // Nothing to retry: the answer holds.
  expect(container.querySelector("button[aria-label$='Retry']")).toBeNull()
  expect(container.textContent).not.toContain("Retry")
})

it("offers to ask again for a picture it could not read now, and shows it when it arrives", async () => {
  let refuse = true
  const read = vi.fn<HeldFileReader>(async (_conversation, file) => {
    if (refuse) throw new HeldFileReadError("unavailable")
    return new Blob([file.digest])
  })
  await render(conversation([chart]), read)
  await settle()
  expect(
    container.querySelector("[data-published-file='chart.png']")?.textContent,
  ).toContain("Couldn’t load")
  refuse = false
  const retry = [
    ...container.querySelectorAll<HTMLButtonElement>(
      "[data-published-file='chart.png'] button",
    ),
  ].find((button) => button.textContent === "Retry")
  if (!retry) throw new Error("no retry")
  await React.act(async () => retry.click())
  await settle()
  expect(container.querySelector("[data-published-file='chart.png'] img")).not.toBeNull()
  // The referenced image's tile offers the same.
  const tile = container.querySelector<HTMLButtonElement>(
    "button[aria-label='WebP image, 3 B: Couldn’t load. Retry']",
  )
  if (!tile) throw new Error("no tile retry")
  await React.act(async () => tile.click())
  await settle()
  expect(
    [...container.querySelectorAll("img")].map((picture) => picture.getAttribute("alt")),
  ).toContain("WebP image, 3 B")
})

it("reads a picture larger than an image the gateway takes only to save it", async () => {
  const read = vi.fn<HeldFileReader>(async () => new Blob(["png"]))
  const poster: PublishedFile = { ...chart, name: "poster.png", size: 64 * 1024 * 1024 }
  await render(conversation([poster]), read)
  await settle()
  expect(container.querySelector("[data-published-file='poster.png'] img")).toBeNull()
  expect(
    container.querySelector("[data-published-file='poster.png'] [data-held-file]"),
  ).toBeNull()
  expect(read.mock.calls.map(([, file]) => file.digest)).not.toContain(poster.digest)
  expect(container.querySelector("button[aria-label='Save poster.png']")).not.toBeNull()
})

it("saves a file through the host's own way of saving, and says when it could not", async () => {
  let fail = false
  const read = vi.fn<HeldFileReader>(async (_conversation, file) => {
    if (fail) throw new HeldFileReadError("unavailable")
    return new Blob([file.digest])
  })
  const saved: [string, string][] = []
  const save = vi.fn<HeldFileSaver>(async (bytes, name) => {
    saved.push([await bytes.text(), name])
    return "saved"
  })
  await render(conversation([report]), read, save)
  await settle()
  // What the transcript painted — the referenced image — is all the page made.
  const painted = [...made]
  const button = container.querySelector<HTMLButtonElement>(
    "button[aria-label='Save report.pdf']",
  )
  if (!button) throw new Error("no save button")
  await React.act(async () => button.click())
  await settle()
  expect(read).toHaveBeenCalledWith(
    "server",
    { digest: report.digest, mimeType: report.mimeType, size: report.size },
    expect.any(AbortSignal),
  )
  // The host was handed the verified bytes under the published name.
  expect(saved).toEqual([[report.digest, "report.pdf"]])
  // Nothing in the page downloads: no link, and no object URL made for it.
  expect(container.querySelector("a[download]")).toBeNull()
  expect(made).toEqual(painted)
  expect(container.querySelector("[role='status']")?.textContent).toBe("")

  // The person closing the dialog is no failure.
  save.mockResolvedValueOnce("cancelled")
  await React.act(async () => button.click())
  await settle()
  expect(container.querySelector("[role='status']")?.textContent).toBe("")

  // A host that saved nothing, and bytes that never came, are both said the same way.
  save.mockRejectedValueOnce({ reason: "not-written", detail: "read-only" })
  await React.act(async () => button.click())
  await settle()
  expect(container.querySelector("[role='status']")?.textContent).toBe("Couldn’t save")
  fail = true
  await React.act(async () => button.click())
  await settle()
  expect(container.querySelector("[role='status']")?.textContent).toBe("Couldn’t save")
  expect(save).toHaveBeenCalledTimes(3)
})

it("reads nothing and offers no save with no reader to read through", async () => {
  await React.act(async () => {
    root.render(
      <Provider store={store as never}>
        <ClockProvider now={() => 1_000}>
          <Transcript
            conversation={conversation([report, chart])}
            ground="paper"
            animateMount={false}
            streamText={false}
            emptyState={false}
            statusLabel="Ready"
            gatewayAvailable
            onOpenPaste={vi.fn()}
          />
        </ClockProvider>
      </Provider>,
    )
  })
  expect(order()).toContain("file:report.pdf")
  expect(container.querySelector("button[aria-label^='Save']")).toBeNull()
  expect(container.querySelector("img")).toBeNull()
})

it("reads afresh through a different reader, letting go of what the old one showed", async () => {
  const first = vi.fn<HeldFileReader>(async () => new Blob(["png"]))
  await render(conversation([chart]), first)
  await settle()
  const shownBefore = [...made]
  const second = vi.fn<HeldFileReader>(async () => new Blob(["png"]))
  await render(conversation([chart]), second)
  await settle()
  expect(second.mock.calls.map(([, file]) => file.digest).sort()).toEqual(
    first.mock.calls.map(([, file]) => file.digest).sort(),
  )
  expect(revoked.sort()).toEqual(shownBefore.sort())
})

it("shows a file published while no turn ran instead of the empty welcome", async () => {
  const read = vi.fn<HeldFileReader>(async () => new Blob(["png"]))
  const empty = { ...conversation([]), turns: [] }
  await render(empty, read, undefined, true)
  // Nothing in it: the welcome.
  expect(container.textContent).toContain("Ready to start")
  await render({ ...conversation([loose]), turns: [] }, read, undefined, true)
  expect(order()).toEqual(["file:notes.txt"])
  expect(container.textContent).not.toContain("Ready to start")
})
