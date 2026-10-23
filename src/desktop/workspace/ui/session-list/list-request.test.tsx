// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { ClockProvider } from "../../../../clock"
import {
  approve,
  arrangementShown,
  followWorkspace,
  loadWorkspace,
  searchList,
  showContent,
  toggleSessionList,
} from "../../adapters/store/commands"
import { listAskIds, onScreen } from "../../application/workspace-state"
import { emptyTranscript, type Approval } from "../../model/transcript"
import {
  fakeSource,
  settle,
  summary,
  testIndex,
  testStore,
  type FakeSource,
} from "../../testing"
import { approvalGist } from "../transcript/approval-card"
import { allowableFromList } from "./list-request"
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

/** The list in three columns over `sessions` added to the test index, each waiting on one review. */
async function listOf(extra: ReturnType<typeof summary>[]) {
  const index = testIndex()
  const source = fakeSource({ ...index, sessions: [...index.sessions, ...extra] })
  for (const session of [...extra, ...index.sessions.filter((s) => s.id === "b")])
    source.transcripts.set(session.id, {
      ...emptyTranscript(session.id),
      revision: 1,
      approvals: [asked(`ask-${session.id}`, `echo ${session.id}`)],
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

const cards = () =>
  [...host.querySelectorAll(".workspace-request-card [data-session-row]")].map((row) =>
    row.getAttribute("data-session-row"),
  )

it("asks only what it draws as cards: never a pinned session, a row its search hides, or a folded list's", async () => {
  const { store, source } = await listOf([
    summary("p", "desktop", 250, "needs-you", { pinned: true }),
  ])
  const state = () => store.getState().workspace
  // Pinned, it is listed under Pinned, a row, and not asked about there.
  expect(cards()).toEqual(["b"])
  expect(listAskIds(state())).toEqual(["b"])
  expect(onScreen(state(), "p")).toBe(false)
  expect(
    await store.dispatch(
      approve({
        sessionId: "p",
        approvalId: "ask-p",
        initiator: "agent",
        scope: "once",
        optionId: "once",
      }),
    ),
  ).toBe("not-asked")
  expect(source.calls.some((call) => call[0] === "approve")).toBe(false)
  // A search that hides it.
  store.dispatch(searchList({ query: "zzz" }))
  expect(listAskIds(state())).toEqual([])
  expect(onScreen(state(), "b")).toBe(false)
  store.dispatch(searchList({ query: "" }))
  expect(listAskIds(state())).toEqual(["b"])
  // A folded list asks nothing.
  store.dispatch(toggleSessionList({ open: false }))
  expect(listAskIds(state())).toEqual([])
})

it("draws as many cards as it asks about, and the rest as rows", async () => {
  const waiting = Array.from({ length: 13 }, (_, at) =>
    summary(`w${at}`, "desktop", 400 + at, "needs-you"),
  )
  const { store } = await listOf(waiting)
  const ids = listAskIds(store.getState().workspace)
  expect(ids).toHaveLength(12)
  expect(cards()).toEqual(ids)
  // Past the bound, a row in the list of cards is one of its items too, a button.
  const group = host.querySelector('[data-list-group="needs-you"]')
  expect(group?.getAttribute("role")).toBe("list")
  expect(group?.querySelectorAll('[role="option"]')).toHaveLength(0)
  for (const row of group?.querySelectorAll("[data-session-row]") ?? [])
    expect(row.closest('[role="listitem"]')).not.toBeNull()
  // The oldest waiting, past the bound, is a row with its words.
  const rest = host.querySelector(
    '[data-list-group="needs-you"] > [data-session-row="b"], [data-list-group="needs-you"] [data-session-row="b"]',
  )
  expect(rest?.closest(".workspace-request-card")).toBeNull()
})

it("keeps a card's controls' keys their own: Home in its field and an arrow on Deny walk nothing", async () => {
  const { store } = await shown()
  const before = store.getState().workspace.panes
  const deny = card()?.querySelector<HTMLButtonElement>('button[data-answer="deny"]')
  deny?.focus()
  await act(async () =>
    deny?.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
    ),
  )
  await act(async () =>
    deny?.dispatchEvent(new KeyboardEvent("keydown", { key: "Home", bubbles: true })),
  )
  expect(store.getState().workspace.panes).toBe(before)
  expect(document.activeElement).toBe(deny)
  // The group of cards is a list, its head a button; the rest stay listboxes of options.
  expect(host.querySelector('[data-list-group="needs-you"]')?.getAttribute("role")).toBe(
    "list",
  )
  expect(
    card()
      ?.closest('[role="listitem"]')
      ?.querySelector('[data-session-row="b"]')
      ?.getAttribute("role"),
  ).toBe("button")
  expect(host.querySelector('[data-list-group="running"]')?.getAttribute("role")).toBe(
    "listbox",
  )
})

it("marks the card answered while its answer is on the way", async () => {
  const source = fakeSource()
  source.hold("approve")
  await shown(source)
  await act(async () =>
    card()?.querySelector<HTMLButtonElement>('button[data-answer="once"]')?.click(),
  )
  expect(card()?.hasAttribute("data-answering")).toBe(true)
  await act(async () => source.release("approve"))
})

it("allows from the list only a request it shows whole, decided by the request, not its layout", () => {
  const shell = (input: Record<string, unknown>) =>
    asked("s", `mcp__nessa__shell ${JSON.stringify(input)}`)
  const whole = (approval: Approval) =>
    allowableFromList(approval, approvalGist(approval))
  expect(whole(shell({ command: "ls -la" }))).toBe(true)
  // Input it would not draw: a working folder, a sandbox switch.
  expect(whole(shell({ command: "ls", cwd: "/", dangerouslyDisableSandbox: true }))).toBe(
    false,
  )
  // More than it draws whole: four lines, or a long line.
  expect(whole(shell({ command: "echo a\necho b\necho c\nrm -rf ~" }))).toBe(false)
  expect(whole(shell({ command: `echo ${"x".repeat(200)}` }))).toBe(false)
  expect(whole(asked("plain", "cargo test"))).toBe(true)
  expect(whole({ ...asked("loading", "mcp__nessa__shell"), loading: "reading" })).toBe(
    false,
  )
})

it("offers a request it cannot show whole its refusals and the chat, never an allow", async () => {
  const review: Approval = {
    ...asked(
      "hidden",
      `mcp__nessa__shell ${JSON.stringify({ command: "ls", cwd: "/" })}`,
    ),
    options: [
      { id: "no", label: "No", choice: "deny" },
      { id: "never", label: "Always deny in this project", choice: "deny" },
      { id: "yes", label: "Yes", choice: "once" },
      { id: "always", label: "Always allow in this project", choice: "always" },
    ],
  }
  await waitingOn({ approvals: [review] })
  // No allow, the other refusal still in its menu, and the chat to read it whole.
  const foot = [
    ...(card()?.querySelectorAll<HTMLButtonElement>(".workspace-request-foot button") ??
      []),
  ].map((button) => button.getAttribute("aria-label") ?? button.textContent)
  expect(foot).toEqual(["No", "More Answers", "Review in Chat"])
  expect(
    card()?.querySelector('button[data-answer="once"], button[data-answer="always"]'),
  ).toBeNull()
})

it("draws a request it may allow whole, and any other as a preview", async () => {
  await waitingOn({ approvals: [asked("short", "cargo test")] })
  expect(
    card()?.querySelector(".workspace-request-command")?.hasAttribute("data-whole"),
  ).toBe(true)
  await act(async () => root.unmount())
  root = createRoot(host)
  await waitingOn({
    approvals: [asked("long", "echo a\necho b\necho c\necho d")],
  })
  expect(
    card()?.querySelector(".workspace-request-command")?.hasAttribute("data-whole"),
  ).toBe(false)
})
