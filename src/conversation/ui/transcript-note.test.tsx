// @vitest-environment jsdom
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import {
  textContent,
  type AgentFeatures,
  type Conversation,
  type Note,
  type Turn,
} from "../model"
import { ClockProvider } from "../../clock"
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

const note = (id: string, change: Partial<Note> = {}): Note => ({
  id,
  text: `Note ${id}`,
  author: { kind: "owner", surface: "desktop" },
  deliveryState: "recorded",
  ...change,
})

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

function conversation(notes: Note[], turns: Turn[]): Conversation {
  return {
    id: "tab",
    serverConversationId: "server",
    title: "Notes",
    phase: "idle",
    draft: [],
    turns,
    remote: {
      artifacts: [],
      notes,
      approvalMode: "ask",
      approvalModes: [{ id: "ask", name: "Provider asks", description: "Asks." }],
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

/** The transcript in order, one entry per bubble or note. */
function order() {
  return [...container.querySelectorAll("[data-note], [data-slot='chat-bubble']")].map(
    (element) =>
      element.hasAttribute("data-note")
        ? `note:${element.getAttribute("data-note")}`
        : (element.textContent ?? "").trim(),
  )
}

const row = (id: string) => container.querySelector(`[data-note="${id}"]`)

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

it("shows each note right after the turn or waiting input it followed, and one left before any turn or after a dropped turn first", async () => {
  const waiting: Turn = {
    id: "three:user",
    from: "user",
    executionId: "three",
    receipt: "queued",
    content: textContent("Third ask"),
  }
  const alsoWaiting: Turn = {
    id: "four:user",
    from: "user",
    executionId: "four",
    receipt: "queued",
    content: textContent("Fourth ask"),
  }
  await render(
    conversation(
      [
        note("first"),
        note("after-one", { afterExecutionId: "one" }),
        // Its turn is older than every turn the bounded view kept.
        note("dropped", { afterExecutionId: "gone" }),
        // Left between two inputs waiting: it follows the first, not both.
        note("between-waiting", { afterExecutionId: "three" }),
        note("after-waiting", { afterExecutionId: "four" }),
        note("after-two", { afterExecutionId: "two" }),
        note("also-after-one", { afterExecutionId: "one" }),
      ],
      [
        ...turn("one", "First ask", "First answer"),
        ...turn("two", "Second ask", "Second answer"),
        waiting,
        alsoWaiting,
      ],
    ),
  )
  expect(order()).toEqual([
    "note:first",
    "note:dropped",
    "First ask",
    "First answer",
    "note:after-one",
    "note:also-after-one",
    "Second ask",
    "Second answer",
    "note:after-two",
    "Third ask",
    "note:between-waiting",
    "Fourth ask",
    "note:after-waiting",
  ])
})

it("labels each note with who left it, from where, and where it stands", async () => {
  await render(
    conversation(
      [
        note("recorded"),
        note("pending", {
          deliveryState: "pending",
          author: { kind: "grant", surface: "phone" },
        }),
        note("assigned", { deliveryState: "assigned" }),
        note("expired", { deliveryState: "retired", retiredReason: "expired" }),
        note("lost", {
          deliveryState: "retired",
          retiredReason: "permission_lost",
          author: { kind: "grant", surface: "phone" },
        }),
      ],
      [],
    ),
  )
  const label = (id: string) => row(id)?.querySelector("[data-note-label]")?.textContent
  const state = (id: string) =>
    row(id)?.querySelector("[data-note-status]")?.textContent ?? null
  expect(label("recorded")).toBe("Note left by the owner, from desktop")
  expect(label("pending")).toBe("Note left by someone it is shared with, from phone")
  // The surface is a name, isolated from the words around it.
  expect(row("pending")?.querySelector("bdi")?.textContent).toBe("phone")
  expect(state("recorded")).toBeNull()
  expect(state("pending")).toBe("Waiting for the next turn")
  expect(state("assigned")).toBe("Given to a turn")
  expect(state("expired")).toBe("Not delivered: expired")
  expect(state("lost")).toBe("Not delivered: sharing changed")
  // Its own row, not a chat bubble and not a turn.
  expect(container.querySelector("[data-slot='chat-bubble']")).toBeNull()
  expect(row("recorded")?.getAttribute("role")).toBe("note")
})

it("shows what a note says as plain text, never as markdown or markup", async () => {
  const text = "**not bold** <b>not markup</b>\n[not a link](https://example.com)"
  await render(conversation([note("plain", { text })], []))
  const said = row("plain")?.querySelector("[data-note-text]")
  expect(said?.textContent).toBe(text)
  expect(said?.querySelector("strong, b, a")).toBeNull()
})

it("is something in the conversation: a note with no turn shows instead of the welcome", async () => {
  await render(conversation([], []), true)
  expect(container.textContent).toContain("Ready to start")
  await render(conversation([note("only")], []), true)
  expect(container.textContent).not.toContain("Ready to start")
  expect(row("only")).not.toBeNull()
})
