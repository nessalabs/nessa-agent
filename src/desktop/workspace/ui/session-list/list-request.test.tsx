// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { ClockProvider } from "../../../../clock"
import {
  arrangementShown,
  followWorkspace,
  loadWorkspace,
  showContent,
} from "../../adapters/store/commands"
import { listAskIds, onScreen } from "../../application/workspace-state"
import { emptyTranscript, type Approval } from "../../model/transcript"
import { fakeSource, settle, testStore, type FakeSource } from "../../testing"
import { approvalGist } from "../transcript/approval-card"
import { SessionList } from "./session-list"

let root: Root
let host: HTMLDivElement
let unfollow: () => void = () => {}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  class Observer {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  Object.assign(globalThis, { ResizeObserver: Observer, IntersectionObserver: Observer })
  Element.prototype.scrollIntoView ??= () => {}
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  unfollow()
  await act(async () => root.unmount())
  host.remove()
})

const asked = (id: string, command: string): Approval => ({
  id,
  command,
  reason: "Runs it.",
  origin: { kind: "agent" },
  options: [
    { id: "deny", label: "Deny", choice: "deny" },
    { id: "always", label: "Always Allow", choice: "always" },
    { id: "once", label: "Allow Once", choice: "once" },
  ],
  ask: "tool",
})

/** The session list in three columns, "b" waiting on two approvals. */
async function shown(source: FakeSource = fakeSource()) {
  source.transcripts.set("b", {
    ...emptyTranscript("b"),
    revision: 1,
    approvals: [
      asked("first", "security import build/nessa-dev.p12 -k release.keychain"),
      asked("second", "rm -rf dist"),
    ],
  })
  const store = testStore(source)
  unfollow = store.dispatch(followWorkspace())
  await store.dispatch(loadWorkspace())
  store.dispatch(arrangementShown({ arrangement: "columns" }))
  await act(async () =>
    root.render(
      <Provider store={store}>
        <ClockProvider now={() => 1_000}>
          <SessionList />
        </ClockProvider>
      </Provider>,
    ),
  )
  await act(async () => settle())
  return { store, source }
}

const card = () =>
  host.querySelector<HTMLElement>('[aria-label="Request from Session b"]')

it("asks a waiting row's first request from the list: who asks, the exact command, its answers, and how many more", async () => {
  const { store } = await shown()
  expect(listAskIds(store.getState().workspace)).toEqual(["b"])
  expect(card()?.querySelector(".workspace-approval-command")?.textContent).toContain(
    "security import build/nessa-dev.p12 -k release.keychain",
  )
  expect(card()?.textContent).toContain("+1 more")
  const answers = [...(card()?.querySelectorAll("button[data-answer]") ?? [])].map(
    (button) => button.getAttribute("data-answer"),
  )
  // Two answers in place; Always waits in the menu beside them.
  expect(answers).toEqual(["deny", "once"])
  // Only the waiting group's rows carry a card.
  expect(host.querySelectorAll(".workspace-request")).toHaveLength(1)
})

it("answers from the list as the transcript's card does, and the card leaves", async () => {
  const { source } = await shown()
  await act(async () =>
    card()?.querySelector<HTMLButtonElement>('button[data-answer="once"]')?.click(),
  )
  await act(async () => settle())
  expect(source.calls).toContainEqual(["approve", "b", "first", "once", "person", "once"])
  // The next request is asked now, with nothing more after it.
  expect(card()?.querySelector(".workspace-approval-command")?.textContent).toContain(
    "rm -rf dist",
  )
  expect(card()?.textContent).not.toContain("more")
})

it("says in place why an answer was refused", async () => {
  const source = fakeSource()
  source.refuse("deny", "not-waiting")
  await shown(source)
  await act(async () =>
    card()?.querySelector<HTMLButtonElement>('button[data-answer="deny"]')?.click(),
  )
  await act(async () => settle())
  expect(card()?.querySelector(".workspace-request-failure")?.textContent).not.toBe("")
  expect(card()?.hasAttribute("data-answering")).toBe(false)
})

it("asks nothing from the list under the overview, or where the layout has no list", async () => {
  const { store } = await shown()
  expect(onScreen(store.getState().workspace, "b")).toBe(true)
  store.dispatch(showContent({ content: "agents" }))
  expect(listAskIds(store.getState().workspace)).toEqual([])
  store.dispatch(showContent({ content: "panes" }))
  store.dispatch(arrangementShown({ arrangement: "sidebar" }))
  expect(listAskIds(store.getState().workspace)).toEqual([])
  expect(onScreen(store.getState().workspace, "b")).toBe(false)
})

/** "b" waiting on one review, as `transcript` holds it. */
async function waitingOn(transcript: Partial<ReturnType<typeof emptyTranscript>>) {
  const source = fakeSource()
  source.transcripts.set("b", { ...emptyTranscript("b"), revision: 1, ...transcript })
  const store = testStore(source)
  unfollow = store.dispatch(followWorkspace())
  await store.dispatch(loadWorkspace())
  store.dispatch(arrangementShown({ arrangement: "columns" }))
  await act(async () =>
    root.render(
      <Provider store={store}>
        <ClockProvider now={() => 1_000}>
          <SessionList />
        </ClockProvider>
      </Provider>,
    ),
  )
  await act(async () => settle())
}

const answersShown = () =>
  [
    ...(card()?.querySelectorAll<HTMLButtonElement>(
      ".workspace-approval-actions button",
    ) ?? []),
  ].map((button) => button.getAttribute("aria-label") ?? button.textContent)

it("reads Claude's shell review as the transcript does, and offers two answers, the rest in its menu", async () => {
  const shell = 'echo "Hello from Nessa – approval test at $(date)"'
  const review: Approval = {
    ...asked("claude", `mcp__nessa__shell ${JSON.stringify({ command: shell })}`),
    options: [
      { id: "no", label: "No", choice: "deny" },
      { id: "never", label: "Always deny in this project", choice: "deny" },
      { id: "project", label: "Always allow in this project", choice: "always" },
      { id: "yes", label: "Yes", choice: "once" },
      {
        id: "shell",
        label: "Yes, and don't ask again for Shell commands",
        choice: "always",
      },
    ],
  }
  await waitingOn({ approvals: [review] })
  // The transcript card's own reading: the shell command, and the tool under it.
  const command = card()?.querySelector(".workspace-approval-command")?.textContent ?? ""
  expect(command).toContain(shell)
  expect(command).not.toContain("mcp__nessa__shell")
  expect(command).toContain(approvalGist(review)?.script ?? "unread")
  expect(card()?.querySelector(".workspace-request-tool")?.textContent).toBe(
    "Tool mcp__nessa__shell",
  )
  expect(answersShown()).toEqual(["No", "Yes", "More Answers"])
})

it("offers a Codex review's refusal and its one-time allow, Always in the menu", async () => {
  await waitingOn({ approvals: [asked("codex", "cargo test")] })
  expect(answersShown()).toEqual(["Deny", "Allow Once", "More Answers"])
})

it("asks a question from the list, held to the card's bound", async () => {
  await waitingOn({
    questions: [
      {
        executionId: "e1",
        questionId: "place",
        message: "Where should this land?",
        questions: [
          {
            key: "where",
            prompt: "Pick a place",
            multiSelect: false,
            freeText: false,
            required: true,
            options: Array.from({ length: 12 }, (_, at) => ({
              value: `p${at}`,
              label: `Place ${at}`,
            })),
          },
        ],
      },
    ],
  })
  const question = host.querySelector<HTMLElement>(
    '[aria-label="Question from Session b"]',
  )
  expect(question?.querySelector('[data-question="place"]')).not.toBeNull()
  expect(question?.dataset.request).toBe("question")
})
