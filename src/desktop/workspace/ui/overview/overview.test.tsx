// @vitest-environment jsdom
/**
 * The Agents overview in a real desktop store over a fake source: each
 * waiting approval shown with its command and answered in place from the
 * keyboard, as the person, through the workspace's own `approve` and `deny`
 * — the keyboard moving on to the next; a refused answer said, and
 * answerable again; the keyboard landing on it as it opens and going back to
 * the focused pane's composer as it leaves; and opening a session putting the
 * overview away.
 */
import { act, StrictMode, type ReactNode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { ClockProvider } from "../../adapters/dom/clock"
import { bidiControls } from "../transcript/bidi-controls.mjs"
import { focusedPaneAttribute } from "../../adapters/dom/focus"
import {
  approve,
  filterOverview,
  showOverviewGroup,
  followWorkspace,
  loadWorkspace,
  setComposerText,
  showContent,
} from "../../adapters/store/commands"
import { selectFocusedSessionId } from "../../adapters/store/selectors"
import {
  controlledAnimationFrames,
  fakeSource,
  keptFilter,
  settle,
  summary,
  testStore,
  type AnimationFrames,
  type FakeSource,
} from "../../testing"
import { selectOverviewOpen } from "../../adapters/store/selectors"
import type { ApprovalAsk, ApprovalOption, ApprovalOrigin } from "../../model/transcript"
import type { WorkspaceIndex } from "../../model/workspace-index"
import { failureCopy, readFailureCopy } from "../failure-copy"
import { OverviewRow } from "../source-list/overview-row"
import { answerPause } from "../../model/overview/walk"
import { peekParts } from "../../model/overview/peek"
import { OverviewLayer } from "./overview-layer"

/** The answers the sample's reviews offer, so the overview's always chord has one to give. */
const sampleAnswers: readonly ApprovalOption[] = [
  { id: "deny", label: "Deny", choice: "deny" },
  { id: "always", label: "Always Allow", choice: "always" },
  { id: "once", label: "Allow Once", choice: "once" },
]

let root: Root
let host: HTMLDivElement
let animation: AnimationFrames

// jsdom stamps a key's `timeStamp` with `Date.now()` when the event is built.
// The overview takes a later answer only once `answerPause` has passed on
// that clock (`takesAnswerKey`). A wall-clock wait is at least that long, so
// under a loaded suite the clock can pass the pause while the test meant to
// stay inside it. This clock moves only when `elapse` says so. A browser
// stamps the same instant on `performance.now()`; one clock for both.
let now = 1_000_000
const elapse = (ms: number) => {
  now += ms
}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  now = 1_000_000
  vi.spyOn(Date, "now").mockImplementation(() => now)
  vi.spyOn(performance, "now").mockImplementation(() => Date.now())
  animation = controlledAnimationFrames()
  Element.prototype.scrollIntoView ??= () => {}
  Element.prototype.scrollTo ??= () => {}
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
  animation.restore()
  vi.restoreAllMocks()
})

/** Two approvals waiting, one session running, one resting. */
function sampleIndex(): WorkspaceIndex {
  return {
    sections: [{ id: "starred", name: "Starred" }],
    channels: [
      { id: "desktop", name: "desktop", sectionId: "starred", private: false, topic: "" },
    ],
    sessions: [
      summary("first", "desktop", 300, "needs-you", { title: "Sign the build" }),
      summary("second", "desktop", 200, "needs-you", { title: "Notarize" }),
      summary("run", "desktop", 250, "running", {
        title: "Split panes",
        now: "Running the pane tests after laying them out as fractions",
      }),
      summary("rest", "desktop", 100),
    ],
  }
}

async function mount({
  strict = false,
  wrap = (tree: ReactNode) => tree,
  index = sampleIndex(),
  secondAsker = { kind: "agent" },
  options = sampleAnswers,
  beforeLoad,
  secondAsks = "tool",
}: {
  strict?: boolean
  wrap?: (tree: ReactNode) => ReactNode
  index?: WorkspaceIndex
  /** Who asks the second session's approval. */
  secondAsker?: ApprovalOrigin
  /** The answers each review offers. */
  options?: readonly ApprovalOption[]
  /** Runs after the source is built, before the workspace reads it. */
  beforeLoad?: (source: FakeSource) => void
  /** What the second session's approval asks. */
  secondAsks?: ApprovalAsk
} = {}) {
  const source = fakeSource(index)
  for (const [sessionId, command] of [
    ["first", "security import build.p12"],
    ["second", "xcrun notarytool submit build.dmg"],
  ] as const) {
    const held = source.transcripts.get(sessionId)
    if (held)
      source.transcripts.set(sessionId, {
        ...held,
        approval: {
          id: `${sessionId}-ask`,
          // An app's review runs the tool it named, as the gateway says it.
          command:
            sessionId === "second" && secondAsker.kind === "app"
              ? `${secondAsker.tool} {}`
              : command,
          reason: `Why ${sessionId}.`,
          origin: sessionId === "second" ? secondAsker : { kind: "agent" },
          options,
          ask: sessionId === "second" ? secondAsks : "tool",
        },
      })
  }
  const running = source.transcripts.get("run")
  if (running)
    source.transcripts.set("run", {
      ...running,
      messages: [
        {
          id: "run-0",
          role: "user",
          at: 800,
          parts: [{ kind: "text", text: "Lay the panes out as fractions." }],
        },
        {
          id: "run-1",
          role: "agent",
          at: 900,
          parts: [
            { kind: "step", step: "read", label: "Read", detail: "panes.tsx" },
            { kind: "step", step: "edit", label: "Edited", detail: "grid.tsx", added: 4 },
            { kind: "text", text: "Laying the panes out as fractions." },
          ],
        },
      ],
      activity: { label: "Running the tests", since: 990 },
    })
  beforeLoad?.(source)
  const filter = keptFilter()
  const store = testStore(source, undefined, filter)
  await store.dispatch(loadWorkspace())
  const scope = { current: host }
  const tree = (
    <Provider store={store}>
      <ClockProvider now={() => 1000}>
        <nav>
          <OverviewRow />
        </nav>
        {/* The focused pane, as the shell draws it: its composer is where the caret goes back. */}
        <main className="workspace-chat" {...{ [focusedPaneAttribute]: "" }}>
          <div className="desktop-composer">
            <textarea aria-label="Message" />
          </div>
        </main>
        {wrap(<OverviewLayer root={scope} />)}
      </ClockProvider>
    </Provider>
  )
  await act(async () => root.render(strict ? <StrictMode>{tree}</StrictMode> : tree))
  await act(async () => settle(10))
  return { source, store, filter }
}

const entry = () =>
  host.querySelector<HTMLButtonElement>(".agents-overview-entry") ?? undefined
const cards = () => [...host.querySelectorAll<HTMLElement>(".agents-request")]
const card = (sessionId: string) =>
  host.querySelector<HTMLElement>(`.agents-request[data-overview-item="${sessionId}"]`)

/** A button in `scope` by the words it shows. */
const button = (scope: Element | null, words: string) =>
  [...(scope?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find((each) =>
    each.textContent?.trim().startsWith(words),
  )

const row = (sessionId: string) =>
  host.querySelector<HTMLElement>(`.agents-row[data-overview-item="${sessionId}"]`)

/** Runs one frame, and the render it asked for. The next frame waits its own turn. */
const nextFrame = () =>
  act(async () => {
    animation.runFrame()
    await settle(10)
  })

/** Opens the overview, and waits out its first frames, after which the keyboard is on it. */
async function open() {
  await act(async () => entry()?.click())
  await act(async () => settle(10))
  for (let frame = 0; frame < 3; frame++) await nextFrame()
}

/** ⌘↩ and the like, as a keyboard off the Mac sends them: Control stands for ⌘. */
async function press(
  target: HTMLElement,
  code: string,
  modifiers: { command?: boolean; alt?: boolean; repeat?: boolean } = {},
) {
  await act(async () => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", {
        code,
        key: code,
        ctrlKey: modifiers.command ?? false,
        altKey: modifiers.alt ?? false,
        repeat: modifiers.repeat ?? false,
        bubbles: true,
        cancelable: true,
      }),
    )
    await settle(10)
  })
}

/** The source moves a session on, as after an answer given elsewhere; a frame on. */
const moveOn = async (
  source: Awaited<ReturnType<typeof mount>>["source"],
  sessionId: string,
  title: string,
  status: "running" | "idle",
) => {
  await act(async () => {
    source.emit({
      kind: "session",
      session: summary(sessionId, "desktop", 600, status, { title, revision: 2 }),
    })
    await settle(10)
  })
  await nextFrame()
}

describe("the agents overview", () => {
  it("leaves Needs you out while nothing waits, and says all is clear only when nothing is listed at all", async () => {
    const quiet = sampleIndex()
    await mount({
      index: {
        ...quiet,
        sessions: quiet.sessions.filter((session) => session.status !== "needs-you"),
      },
    })
    await open()
    expect(host.querySelector("#agents-needs-you")).toBeNull()
    expect(host.querySelector(".agents-clear-title")).toBeNull()
    expect(host.querySelector("#agents-working")).not.toBeNull()
  })

  it("says all is clear when nothing at all is listed", async () => {
    await mount({ index: { ...sampleIndex(), sessions: [] } })
    await open()
    expect(host.querySelector("#agents-needs-you")).toBeNull()
    expect(host.querySelector(".agents-clear-title")?.textContent).toBe(
      "Nothing needs you",
    )
  })

  it("shows what each waiting agent asks, and what is working, over panes left in place", async () => {
    await mount()
    await open()
    expect(cards().map((each) => each.dataset.overviewItem)).toEqual(["first", "second"])
    expect(card("first")?.querySelector(".agents-request-command")?.textContent).toBe(
      "security import build.p12",
    )
    expect(host.querySelector(".agents-row")?.textContent).toContain("Split panes")
    expect(host.querySelector(".agents-overview-header p")?.textContent).toBe(
      "2 need you · 1 working",
    )
    // The panes are kept, laid out, under the overview.
    expect(host.querySelector('textarea[aria-label="Message"]')).not.toBeNull()
  })

  it("O3 (#436): a row says who asks — an app's request names the app, not the agent", async () => {
    await mount({
      secondAsker: { kind: "app", server: "mcptest", tool: "app_delete_row" },
    })
    await open()
    const label = (id: string) => card(id)?.getAttribute("aria-label") ?? ""
    const agent = /\. (.*) wants to run security import build\.p12\.$/.exec(
      label("first"),
    )?.[1]
    expect(agent).toBeTruthy()
    // The app's server isolated from the words around it, as on the card.
    expect(label("second")).toMatch(
      /\. The \u2068mcptest\u2069 app wants to run \u2068app_delete_row \{\}\u2069\.$/,
    )
    expect(label("second")).not.toContain(`${agent} wants`)
  })

  it("E2-1 (#390): a row shows the bidi controls an app's names carry as U+FFFD, in its words and its accessible name, each name and the app's command isolated", async () => {
    // The reviewer's strings: a stray PDI then an embedding; an override.
    const server = "a\u2069\u202Eb"
    const tool = "c\u2069\u2069\u202Bd"
    await mount({ secondAsker: { kind: "app", server, tool } })
    await open()
    expect(card("second")?.getAttribute("aria-label")).toMatch(
      /\. The \u2068a\uFFFD\uFFFDb\u2069 app wants to run \u2068c\uFFFD\uFFFD\uFFFDd \{\}\u2069\.$/,
    )
    const command = card("second")?.querySelector<HTMLElement>(".agents-request-command")
    expect(command?.querySelector("bdi")?.textContent).toBe("c\uFFFD\uFFFD\uFFFDd {}")
    expect(command?.title).toBe("c\uFFFD\uFFFD\uFFFDd {}")
  })

  it("E2-1 (#390): a row for an app's message shows an override in its server's name as U+FFFD", async () => {
    const server = "evil\u202Egnp.exe"
    const tool = "show_rows"
    const reason = `The ${tool} app on ${server} asks to send a message as you`
    await mount({
      secondAsker: { kind: "app", server, tool },
      secondAsks: "message",
      beforeLoad: (source) => {
        const held = source.transcripts.get("second")
        if (!held?.approval) return
        source.transcripts.set("second", {
          ...held,
          approval: { ...held.approval, reason },
        })
      },
    })
    await open()
    expect(card("second")?.getAttribute("aria-label")).toMatch(
      /\. The \u2068evil\uFFFDgnp\.exe\u2069 app wants to send a message as you\.$/,
    )
    // The gateway's title repeats the server. Nothing in the row carries a control it brought.
    expect(card("second")?.querySelector(".agents-request-why")?.textContent).toBe(
      "The show_rows app on evil\uFFFDgnp.exe asks to send a message as you",
    )
    expect(card("second")?.textContent).not.toMatch(bidiControls)
    await act(async () => card("second")?.click())
    await act(async () => settle(10))
    const peek = host.querySelector(".agents-inline-peek")
    expect(peek?.querySelector(".agents-peek-reason")?.textContent).toBe(
      "The show_rows app on evil\uFFFDgnp.exe asks to send a message as you",
    )
    expect(peek?.querySelector(".workspace-approval-command bdi")?.textContent).toBe(
      "show_rows",
    )
  })

  it("D19 (#390): a row says an app asks to send a message as the person, not to run its tool", async () => {
    await mount({
      secondAsker: { kind: "app", server: "mcptest", tool: "show_rows" },
      secondAsks: "message",
    })
    await open()
    expect(card("second")?.getAttribute("aria-label")).toMatch(
      /\. The \u2068mcptest\u2069 app wants to send a message as you\.$/,
    )
    expect(card("first")?.getAttribute("aria-label")).toMatch(
      / wants to run security import build\.p12\.$/,
    )
  })

  it("D19 (#390): a message's answers say it is sent, not run, in the row and in its peek; a tool's say it is run", async () => {
    await mount({
      secondAsker: { kind: "app", server: "mcptest", tool: "show_rows" },
      secondAsks: "message",
    })
    await open()
    const tips = (scope: Element | null | undefined) =>
      [...(scope?.querySelectorAll<HTMLElement>("button[data-tooltip]") ?? [])]
        .map((each) => each.dataset.tooltip ?? "")
        .filter((tip) => / it/.test(tip))
    expect(tips(card("second"))).toEqual([
      "Don’t send it",
      "Send it once. Hold ⌥ to always allow it",
    ])
    expect(tips(card("first"))).toEqual([
      "Don’t run it",
      "Run it once. Hold ⌥ to always allow it",
    ])
    await act(async () => card("second")?.click())
    await act(async () => settle(10))
    const peek = host.querySelector(".agents-inline-peek .agents-peek-ask")
    expect(tips(peek)).toEqual([
      "Don’t send it",
      "Allow it now, and whenever it’s asked again",
      "Send it once",
    ])
    await act(async () => card("first")?.click())
    await act(async () => settle(10))
    expect(tips(host.querySelector(".agents-inline-peek .agents-peek-ask"))).toEqual([
      "Don’t run it",
      "Allow it now, and whenever it’s asked again",
      "Run it once",
    ])
  })

  it("answers from the keyboard as the person, and moves on to the next request", async () => {
    const { source } = await mount()
    await open()
    const first = card("first")
    expect(document.activeElement).toBe(first)
    await press(first as HTMLElement, "Enter", { command: true })
    expect(source.calls).toContainEqual([
      "approve",
      "first",
      "first-ask",
      "once",
      "person",
      "once",
    ])
    expect(document.activeElement).toBe(card("second"))
    // Held in place while it settles, saying what became of it.
    expect(card("first")?.dataset.phase).toBe("settled")
    // A new press, once the person can see where the keyboard went.
    elapse(answerPause)
    await press(card("second") as HTMLElement, "Backspace", { command: true })
    expect(source.calls).toContainEqual([
      "deny",
      "second",
      "second-ask",
      "person",
      "deny",
    ])
  })

  it("answers one request per press: a held key's repeats, or a press straight after, answer nothing more", async () => {
    const { source } = await mount()
    await open()
    await press(card("first") as HTMLElement, "Enter", { command: true })
    expect(document.activeElement).toBe(card("second"))
    // The same key held: its repeats land on the next request and answer nothing.
    for (let repeat = 0; repeat < 4; repeat++)
      await press(document.activeElement as HTMLElement, "Enter", {
        command: true,
        repeat: true,
      })
    // A second press 80ms after the first: before the person could see where the keyboard went.
    elapse(80)
    await press(document.activeElement as HTMLElement, "Enter", { command: true })
    // Nor does Return open it.
    await press(document.activeElement as HTMLElement, "Enter")
    const answers = source.calls.filter(
      (call) => call[0] === "approve" || call[0] === "deny",
    )
    expect(answers).toEqual([["approve", "first", "first-ask", "once", "person", "once"]])
    expect(document.activeElement).toBe(card("second"))
    expect(host.querySelector('[data-content="panes"]')).toBeNull()
  })

  it("measures the pause from when a key was pressed, not from when the page got to it", async () => {
    const { source } = await mount()
    await open()
    await press(card("first") as HTMLElement, "Enter", { command: true })
    // Pressed straight after the answer, and handled only once the pause has
    // passed — the page busy meanwhile: still too soon to be a choice.
    const early = new KeyboardEvent("keydown", {
      code: "Enter",
      key: "Enter",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    })
    elapse(answerPause + 20)
    await act(async () => {
      document.activeElement?.dispatchEvent(early)
      await settle(10)
    })
    const answers = source.calls.filter(
      (call) => call[0] === "approve" || call[0] === "deny",
    )
    expect(answers).toEqual([["approve", "first", "first-ask", "once", "person", "once"]])
  })

  it("times the pause from when the answering key was pressed, not from when the page got to it", async () => {
    const { source } = await mount()
    await open()
    const key = () =>
      new KeyboardEvent("keydown", {
        code: "Enter",
        key: "Enter",
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      })
    // The answer is pressed; the page gets to it only after the pause, and a
    // second press was made that long after the first: a choice, and taken.
    const answer = key()
    elapse(answerPause + 30)
    const next = key()
    await act(async () => {
      card("first")?.dispatchEvent(answer)
      await settle(10)
      document.activeElement?.dispatchEvent(next)
      await settle(10)
    })
    const answers = source.calls.filter(
      (call) => call[0] === "approve" || call[0] === "deny",
    )
    expect(answers).toEqual([
      ["approve", "first", "first-ask", "once", "person", "once"],
      ["approve", "second", "second-ask", "once", "person", "once"],
    ])
  })

  it("allows always with ⌥⌘↩", async () => {
    const { source } = await mount()
    await open()
    await press(card("first") as HTMLElement, "Enter", { command: true, alt: true })
    expect(source.calls).toContainEqual([
      "approve",
      "first",
      "first-ask",
      "always",
      "person",
      "always",
    ])
  })

  it("shares one answer with a pane: an answer on its way from anywhere rests its buttons", async () => {
    const { source, store } = await mount()
    await open()
    source.hold("approve")
    // As the pane's card would, or an agent.
    let fromPane: Promise<unknown> = Promise.resolve()
    await act(async () => {
      fromPane = store.dispatch(
        approve({
          sessionId: "first",
          approvalId: "first-ask",
          initiator: "agent",
          optionId: "once",
        }),
      )
      await settle(10)
    })
    expect(button(card("first"), "Allow")?.disabled).toBe(true)
    await press(card("first") as HTMLElement, "Enter", { command: true })
    expect(source.calls.filter((call) => call[0] === "approve")).toHaveLength(1)
    await act(async () => source.release("approve"))
    expect(await fromPane).toBe("sent")
  })

  it("says what a conversation could not be read as, on the request and in its peek", async () => {
    await mount({
      beforeLoad: (source) => source.refuse("transcript", "unavailable"),
    })
    await open()
    const unread = "Nessa couldn’t read this conversation just now."
    expect(readFailureCopy("unavailable", "conversation")).toBe(unread)
    expect(failureCopy("unavailable")).not.toBe(unread)
    for (const id of ["first", "second"])
      expect(card(id)?.querySelector(".agents-request-failure")?.textContent).toBe(unread)
    await act(async () => card("first")?.click())
    await act(async () => settle(10))
    expect(
      host.querySelector(".agents-inline-peek .agents-peek-failure")?.textContent,
    ).toBe(unread)
  })

  it("says why an answer was not confirmed, and lets the person answer again", async () => {
    const { source } = await mount()
    source.refuse("approve", "unavailable")
    await open()
    await press(card("first") as HTMLElement, "Enter", { command: true })
    expect(card("first")?.dataset.phase).toBeUndefined()
    expect(card("first")?.querySelector(".agents-request-failure")?.textContent).toBe(
      "Nessa couldn’t confirm this just now.",
    )
    source.refuse("approve", undefined)
    const allow = button(card("first"), "Allow")
    expect(allow?.disabled).toBe(false)
    await act(async () => allow?.click())
    await act(async () => settle(10))
    expect(
      source.calls.filter((call) => call[0] === "approve" && call[1] === "first"),
    ).toHaveLength(2)
  })

  it("walks the list with the arrow keys", async () => {
    await mount()
    await open()
    await press(card("first") as HTMLElement, "ArrowDown")
    expect(document.activeElement).toBe(card("second"))
    await press(card("second") as HTMLElement, "ArrowDown")
    expect(document.activeElement?.textContent).toContain("Split panes")
    await press(document.activeElement as HTMLElement, "Home")
    expect(document.activeElement).toBe(card("first"))
  })

  it("opens a session in the workspace and gives the chat area back", async () => {
    const { store } = await mount()
    await open()
    await press(card("second") as HTMLElement, "Enter")
    expect(selectFocusedSessionId(store.getState())).toBe("second")
    expect(host.querySelector(".agents-overview")).toBeNull()
  })

  it("goes back to the workspace on Escape, the keyboard to the focused pane's composer", async () => {
    await mount()
    await open()
    await press(card("first") as HTMLElement, "Escape")
    expect(host.querySelector(".agents-overview")).toBeNull()
    await nextFrame()
    expect(document.activeElement).toBe(
      host.querySelector('textarea[aria-label="Message"]'),
    )
  })

  it("puts the keyboard on the current row when opened, and so the peek's session is chosen", async () => {
    const { store } = await mount()
    await open()
    expect(document.activeElement).toBe(card("first"))
    expect(store.getState().workspace.overview.selected).toBe("first")
  })

  it("puts the keyboard on the current row under StrictMode too, whose second mount cancels the first try", async () => {
    await mount({ strict: true })
    await open()
    expect(document.activeElement).toBe(card("first"))
  })

  it("puts the keyboard on the current row when an agent opens it", async () => {
    const { store } = await mount()
    await act(async () => store.dispatch(showContent({ content: "agents" })))
    for (let frame = 0; frame < 3; frame++) await nextFrame()
    expect(document.activeElement).toBe(card("first"))
  })

  it("moves the keyboard to the next request after a click on Allow in the peek", async () => {
    const { source } = await mount()
    await open()
    await act(async () => card("first")?.click())
    await act(async () => settle(10))
    const peek = host.querySelector(".agents-inline-peek")
    const allow = button(peek, "Allow Once")
    allow?.focus()
    await act(async () => {
      allow?.click()
      await settle(10)
    })
    expect(source.calls).toContainEqual([
      "approve",
      "first",
      "first-ask",
      "once",
      "person",
      "once",
    ])
    expect(document.activeElement).toBe(card("second"))
  })

  it("peeks at a session where it is listed, without leaving the overview", async () => {
    await mount()
    await open()
    await act(async () => row("run")?.click())
    await act(async () => settle(10))
    const peek = host.querySelector(".agents-inline-peek")
    expect(peek?.textContent).toContain("Running the tests")
    expect(peek?.textContent).toContain("grid.tsx")
    expect(peek?.textContent).toContain("Laying the panes out as fractions.")
    expect(host.querySelector(".agents-overview")).not.toBeNull()
    // Chosen again, it closes.
    await act(async () => row("run")?.click())
    expect(host.querySelector(".agents-inline-peek")).toBeNull()
  })

  it("opens a session on a double-click, and on ↩", async () => {
    const { store } = await mount()
    await open()
    await act(async () => {
      row("run")?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }))
    })
    expect(selectFocusedSessionId(store.getState())).toBe("run")
    expect(host.querySelector(".agents-overview")).toBeNull()
  })

  it("draws the command in order on the row and in the peek (#553)", async () => {
    const argument = "\u202Emoc.live@bob\u202C"
    const command = `send ${JSON.stringify({ to: argument })}`
    await mount({
      beforeLoad: (source) => {
        const held = source.transcripts.get("first")
        if (!held?.approval) throw new Error("no approval")
        source.transcripts.set("first", {
          ...held,
          approval: { ...held.approval, command },
        })
      },
    })
    await open()
    const row = card("first")
    const drawn = row?.querySelector(".agents-request-command")
    expect(drawn?.textContent).not.toMatch(bidiControls)
    expect(drawn?.textContent?.indexOf("moc.live@bob")).toBeGreaterThan(
      drawn?.textContent?.indexOf("\\u202e") ?? -1,
    )
    expect(drawn?.getAttribute("title")).toBe(drawn?.textContent)
    expect(row?.getAttribute("aria-label")).toContain("\\u202e")
    expect(row?.getAttribute("aria-label")).not.toMatch(bidiControls)
    await act(async () => row?.click())
    await act(async () => settle(10))
    const peek = host.querySelector(".agents-inline-peek .workspace-approval-command")
    const peekText = peek?.textContent?.replace(/^\$ /, "") ?? ""
    expect(peekText).toBe(drawn?.textContent)
    expect(JSON.parse(peekText.slice(peekText.indexOf(" ")))).toEqual({ to: argument })
  })

  it("does not offer always when the review does not (#444)", async () => {
    const { source } = await mount({
      options: [
        { id: "deny", label: "Deny", choice: "deny" },
        { id: "allow", label: "Allow", choice: "once" },
      ],
    })
    await open()
    const row = card("first")
    expect(row?.querySelector("[data-answer='always']")).toBeNull()
    expect(row?.textContent).not.toContain("Always Allow")
    expect(row?.hasAttribute("data-offers-always")).toBe(false)
    // ⌥⌘↩ is always, which this review does not offer: it answers nothing.
    await press(row as HTMLElement, "Enter", { command: true, alt: true })
    expect(
      source.calls.filter((call) => call[0] === "approve" || call[0] === "deny"),
    ).toEqual([])
    // ⌥ on Allow does not invent an always answer either: it allows this request.
    await act(async () => {
      button(row, "Allow")?.dispatchEvent(
        new MouseEvent("click", { bubbles: true, altKey: true }),
      )
      await settle(10)
    })
    expect(source.calls.filter((call) => call[2] === "first-ask")).toEqual([
      ["approve", "first", "first-ask", "once", "person", "allow"],
    ])
  })

  it("allows always when Allow is clicked with ⌥ held", async () => {
    const { source } = await mount()
    await open()
    await act(async () => {
      button(card("first"), "Allow")?.dispatchEvent(
        new MouseEvent("click", { bubbles: true, altKey: true }),
      )
      await settle(10)
    })
    expect(source.calls).toContainEqual([
      "approve",
      "first",
      "first-ask",
      "always",
      "person",
      "always",
    ])
  })

  it("rests on the list once the last request is answered, rather than jumping away", async () => {
    await mount()
    await open()
    await press(card("first") as HTMLElement, "Enter", { command: true })
    elapse(answerPause)
    await press(card("second") as HTMLElement, "Enter", { command: true })
    expect(document.activeElement?.classList.contains("agents-overview-column")).toBe(
      true,
    )
  })

  it("lists only what is ongoing at first, and says how many it keeps out", async () => {
    await mount()
    await open()
    expect(row("rest")).toBeNull()
    expect(host.querySelector(".agents-overview-resting")?.textContent).toContain(
      "1 more session outside this view",
    )
  })

  it("lists every session once All is chosen, counts what it lists, and keeps the choice", async () => {
    const { store, filter } = await mount()
    await open()
    await act(async () => {
      store.dispatch(filterOverview({ filter: { scope: "all", range: "any", tags: [] } }))
    })
    expect(filter.writes.at(-1)).toEqual({ scope: "all", range: "any", tags: [] })
    expect(row("rest")).not.toBeNull()
    expect(host.querySelector(".agents-overview-header p")?.textContent).toBe(
      "2 need you · 1 working · 1 earlier",
    )
    expect(host.querySelector(".agents-filter")?.textContent).toBe("All")
  })

  it("replies from the peek with ⌘R, through the workspace's send, as the person", async () => {
    const { source, store } = await mount()
    await open()
    await press(card("second") as HTMLElement, "KeyR", { command: true })
    await nextFrame()
    const field = host.querySelector<HTMLTextAreaElement>(
      '[data-reply-for="second"] textarea',
    )
    expect(field?.placeholder).toBe("Reply to Claude…")
    expect(document.activeElement).toBe(field)
    await act(async () =>
      store.dispatch(
        setComposerText({ sessionId: "second", text: "Use the beta profile." }),
      ),
    )
    await press(field as HTMLElement, "Enter")
    const sent = source.calls.find((call) => call[0] === "send")?.[1]
    expect(sent).toMatchObject({
      sessionId: "second",
      text: "Use the beta profile.",
      initiator: "person",
    })
    // Emptied by the workspace's send, and still where the person is writing.
    expect(store.getState().workspace.composerText).not.toHaveProperty("second")
    expect(field?.value).toBe("")
    expect(document.activeElement).toBe(field)
    // Escape gives the keyboard back to the list, on the row it replied to.
    await press(field as HTMLElement, "Escape")
    expect(document.activeElement).toBe(card("second"))
    expect(host.querySelector(".agents-overview")).not.toBeNull()
  })
})

describe("the counts show one group alone", () => {
  const counts = () => [
    ...host.querySelectorAll<HTMLButtonElement>(".agents-overview-counts button"),
  ]
  const count = (words: string) =>
    counts().find((each) => each.textContent?.trim() === words)
  const pressed = () =>
    counts()
      .filter((each) => each.getAttribute("aria-pressed") === "true")
      .map((each) => each.textContent)

  it("are toggle buttons in a group the keyboard reaches, none pressed at first", async () => {
    await mount()
    await open()
    const line = host.querySelector(".agents-overview-counts")
    expect(line?.getAttribute("role")).toBe("group")
    expect(line?.getAttribute("aria-label")).toBe("Show only")
    expect(counts().map((each) => [each.textContent, each.type, each.tabIndex])).toEqual([
      ["2 need you", "button", 0],
      ["1 working", "button", 0],
    ])
    expect(counts().every((each) => each.getAttribute("aria-pressed") === "false")).toBe(
      true,
    )
  })

  it("shows only a count's group, pressed, and every group once it is chosen again", async () => {
    const { store } = await mount()
    await open()
    await act(async () => count("1 working")?.click())
    expect(cards()).toEqual([])
    expect(row("run")).not.toBeNull()
    expect(host.querySelector("#agents-needs-you")).toBeNull()
    expect(pressed()).toEqual(["1 working"])
    // Every count stays, so another group can be chosen from the same line.
    expect(counts().map((each) => each.textContent)).toEqual(["2 need you", "1 working"])
    expect(store.getState().workspace.overview.group).toBe("working")
    await act(async () => count("1 working")?.click())
    expect(cards()).toHaveLength(2)
    expect(pressed()).toEqual([])
  })

  it("moves from one group to another in a click", async () => {
    await mount()
    await open()
    await act(async () => count("1 working")?.click())
    await act(async () => count("2 need you")?.click())
    expect(cards()).toHaveLength(2)
    expect(row("run")).toBeNull()
    expect(pressed()).toEqual(["2 need you"])
  })

  it("lets Escape show every group first, and leave on the next", async () => {
    const { store } = await mount()
    await open()
    await act(async () => count("1 working")?.click())
    const focused = count("1 working") as HTMLElement
    focused.focus()
    await press(focused, "Escape")
    expect(selectOverviewOpen(store.getState())).toBe(true)
    expect(store.getState().workspace.overview.group).toBeNull()
    expect(cards()).toHaveLength(2)
    await press(document.activeElement as HTMLElement, "Escape")
    expect(selectOverviewOpen(store.getState())).toBe(false)
  })

  it("keeps a group that lists nothing in the line at nought, says so, and Show All lists what the filter kept out of it", async () => {
    const { store, filter } = await mount()
    await open()
    await act(async () => store.dispatch(showOverviewGroup({ group: "earlier" })))
    expect(pressed()).toEqual(["0 earlier"])
    const resting = () =>
      [...host.querySelectorAll(".agents-overview-resting")].map((line) =>
        line.textContent?.trim(),
      )
    expect(resting()).toEqual([
      "Nothing from earlier",
      "1 more session outside this view · Show All",
    ])
    await act(async () => button(host, "Show All")?.click())
    expect(filter.writes.at(-1)).toEqual({ scope: "all", range: "any", tags: [] })
    expect(row("rest")).not.toBeNull()
    expect(pressed()).toEqual(["1 earlier"])
    expect(resting()).toEqual([])
  })

  const headings = () =>
    [
      ...host.querySelectorAll(".agents-overview-column h2, .agents-overview-resting"),
    ].map((each) => each.textContent?.trim())
  const inList = () =>
    host.querySelector(".agents-overview-column")?.contains(document.activeElement) ??
    false

  it("says nothing needs you once its last request moves on, the one looked at still chosen beneath, under Working", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    await act(async () => count("2 need you")?.click())
    expect(store.getState().workspace.overview.selected).toBe("first")
    await moveOn(source, "second", "Notarize", "running")
    await moveOn(source, "first", "Sign the build", "running")
    expect(pressed()).toEqual(["0 need you"])
    expect(headings()).toEqual(["Nothing needs you", "Working"])
    expect(cards()).toEqual([])
    expect(row("first")).not.toBeNull()
    expect(row("second")).toBeNull()
    expect(store.getState().workspace.overview).toMatchObject({
      group: "needsYou",
      selected: "first",
    })
  })

  it("counts only what the filter lets through: the session looked at, kept out by it, is listed and not counted", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    await act(async () => row("run")?.click())
    // Ongoing keeps out what is idle and seen: "run" finishes, looked at.
    await moveOn(source, "run", "Split panes", "idle")
    expect(row("run")).not.toBeNull()
    expect(headings()).toContain("Earlier")
    expect(counts().map((each) => each.textContent)).toEqual(["2 need you"])
  })

  it("puts the keyboard on the list when the count it is on goes: let go at nought", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    await act(async () => count("1 working")?.click())
    await moveOn(source, "run", "Split panes", "idle")
    const nought = count("0 working") as HTMLButtonElement
    nought.focus()
    await act(async () => nought.click())
    await nextFrame()
    expect(count("0 working")).toBeUndefined()
    expect(inList()).toBe(true)
    // And the arrows walk the list from there.
    const from = document.activeElement as HTMLElement
    await press(from, "ArrowUp")
    expect(inList()).toBe(true)
    expect(document.activeElement).not.toBe(from)
  })

  it("puts the keyboard on the list when the count it is on goes: emptied by the source", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    const working = count("1 working") as HTMLButtonElement
    working.focus()
    await moveOn(source, "run", "Split panes", "idle")
    expect(count("1 working")).toBeUndefined()
    expect(inList()).toBe(true)
  })

  it("leaves the keyboard where it is when a count it is not on goes", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    const needs = count("2 need you") as HTMLButtonElement
    needs.focus()
    await moveOn(source, "run", "Split panes", "idle")
    expect(document.activeElement).toBe(needs)
  })

  it("says nothing is kept out of a group the filter keeps nothing out of", async () => {
    await mount()
    await open()
    // Ongoing keeps out the idle session, which is not working.
    expect(host.querySelector(".agents-overview-resting")).not.toBeNull()
    await act(async () => count("1 working")?.click())
    expect(host.querySelector(".agents-overview-resting")).toBeNull()
  })
})

describe("the peek tells the turn's story", () => {
  it("from the person's latest message, through the agent's work in order, to what it does now", async () => {
    await mount()
    await open()
    await act(async () => row("run")?.click())
    await act(async () => settle(10))
    const story = host.querySelector(".agents-inline-peek .agents-peek-story")
    const told = [...(story?.children ?? [])].map((part) =>
      part.classList.contains("workspace-live")
        ? "live"
        : `${part.getAttribute("data-role")}: ${part.textContent?.trim().slice(0, 12)}`,
    )
    expect(told).toEqual(["user: Lay the pane", "agent: Readpanes.ts", "live"])
    const work = story?.querySelector('[data-role="agent"]')
    expect(
      [...(work?.querySelectorAll(".workspace-steps li, p") ?? [])].map((part) =>
        part.textContent?.trim(),
      ),
    ).toEqual(["Readpanes.tsx", "Editedgrid.tsx+4", "Laying the panes out as fractions."])
  })

  it("says what is going on in the source's own line, and nothing where the source says nothing", async () => {
    await mount()
    await open()
    await act(async () => row("run")?.click())
    await act(async () => settle(10))
    expect(host.querySelector(".agents-peek-summary")?.textContent).toBe(
      "Running the pane tests after laying them out as fractions",
    )
    await act(async () => row("run")?.click())
    await act(async () => card("first")?.click())
    await act(async () => settle(10))
    expect(host.querySelector(".agents-inline-peek")).not.toBeNull()
    expect(host.querySelector(".agents-peek-summary")).toBeNull()
  })

  it("draws no line where the source's line is only whitespace", async () => {
    const index = sampleIndex()
    await mount({
      index: {
        ...index,
        sessions: index.sessions.map((each) =>
          each.id === "run" ? { ...each, now: " \n\t" } : each,
        ),
      },
    })
    await open()
    await act(async () => row("run")?.click())
    await act(async () => settle(10))
    expect(host.querySelector(".agents-inline-peek")).not.toBeNull()
    expect(host.querySelector(".agents-peek-summary")).toBeNull()
  })
})

describe("the peek's story is bounded, and scrolls from the keyboard beneath its row", () => {
  const longTurn = (source: Awaited<ReturnType<typeof mount>>["source"]) => {
    const held = source.transcripts.get("run")
    if (!held) return
    source.transcripts.set("run", {
      ...held,
      messages: [
        {
          id: "run-0",
          role: "user",
          at: 800,
          parts: [{ kind: "text", text: "Lay the panes out as fractions." }],
        },
        ...Array.from({ length: 4 }, (_, message) => ({
          id: `run-${message + 1}`,
          role: "agent" as const,
          at: 900 + message,
          parts: Array.from({ length: 10 }, (_, index) => ({
            kind: "step" as const,
            step: "read" as const,
            label: "Read",
            detail: `file-${message}-${index}.ts`,
          })),
        })),
      ],
    })
  }

  it("draws the turn's latest parts after the person's message, says there is more above, and opens the session from there", async () => {
    const { source, store } = await mount()
    longTurn(source)
    await open()
    await act(async () => row("run")?.click())
    await act(async () => settle(10))
    const story = host.querySelector(".agents-inline-peek .agents-peek-story")
    const steps = [...(story?.querySelectorAll(".workspace-steps li") ?? [])]
    expect(steps).toHaveLength(peekParts)
    expect(steps.at(-1)?.textContent).toBe("Readfile-3-9.ts")
    expect(story?.children[0].getAttribute("data-role")).toBe("user")
    const earlier = story?.children[1]
    expect(earlier?.textContent).toBe("Earlier in this turn · Open")
    await act(async () => button(earlier ?? null, "Open")?.click())
    expect(selectFocusedSessionId(store.getState())).toBe("run")
    expect(selectOverviewOpen(store.getState())).toBe(false)
  })

  it("says nothing of more above a turn that fits", async () => {
    await mount()
    await open()
    await act(async () => row("run")?.click())
    await act(async () => settle(10))
    expect(host.querySelector(".agents-peek-earlier")).toBeNull()
  })

  it("takes the keyboard beneath a row, and keeps the keys that scroll it rather than walking the list", async () => {
    await mount()
    await open()
    await act(async () => row("run")?.click())
    await act(async () => settle(10))
    const story = host.querySelector<HTMLElement>(
      ".agents-inline-peek .agents-peek-story",
    )
    expect(story?.tabIndex).toBe(0)
    story?.focus()
    for (const key of ["ArrowDown", "ArrowUp", "End", "Home"]) {
      await press(story as HTMLElement, key)
      expect(document.activeElement).toBe(story)
    }
  })
})

describe("a row keeps the keyboard as its session changes group", () => {
  // The window has focus, as the person's does; jsdom says it has only while
  // an element does. A test that sends it away says so.
  let windowFocused = true
  beforeEach(() => {
    windowFocused = true
    vi.spyOn(document, "hasFocus").mockImplementation(() => windowFocused)
  })
  afterEach(() => vi.restoreAllMocks())
  /** A click on plain text: a press, then focus to the page's body. */
  const clickAway = (from: HTMLElement) => {
    document.body.dispatchEvent(new Event("mousedown", { bubbles: true }))
    from.blur()
  }
  const heading = (sessionId: string) =>
    row(sessionId)?.closest(".agents-overview-group")?.querySelector("h2")?.textContent
  it("follows its session to its new row, and the arrows walk the list from there", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    const before = row("run") as HTMLElement
    await act(async () => before.focus())
    expect(heading("run")).toBe("Working")
    // Its turn over, looked at: the session rests under Earlier, its row drawn anew.
    await moveOn(source, "run", "Split panes", "idle")
    const after = row("run")
    expect(heading("run")).toBe("Earlier")
    expect(after).not.toBe(before)
    expect(document.activeElement).toBe(after)
    await press(after as HTMLElement, "ArrowUp")
    expect(document.activeElement).toBe(card("second"))
  })

  it("leaves the keyboard where it is when a row it is not on changes group", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    const first = card("first") as HTMLElement
    await act(async () => first.focus())
    // Not looked at, it leaves Working (the filter may keep it out altogether).
    await moveOn(source, "run", "Split panes", "idle")
    expect(heading("run")).not.toBe("Working")
    expect(document.activeElement).toBe(first)
  })

  it("goes to the row the list then chooses when its session is removed, never to the page", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    await act(async () => row("run")?.focus())
    await act(async () => {
      source.emit({ kind: "session-removed", sessionId: "run", revision: 9 })
      await settle(10)
    })
    await nextFrame()
    expect(row("run")).toBeNull()
    // The list keeps the first it lists chosen (`keepOverviewChoice`): its row, not the bare list.
    expect(document.activeElement).toBe(card("first"))
    await press(card("first") as HTMLElement, "ArrowDown")
    expect(document.activeElement).toBe(card("second"))
  })

  it("follows its session from the peek beneath its row", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    await act(async () => row("run")?.click())
    await act(async () => settle(10))
    const story = host.querySelector<HTMLElement>(
      ".agents-inline-peek .agents-peek-story",
    )
    await act(async () => story?.focus())
    expect(document.activeElement).toBe(story)
    await moveOn(source, "run", "Split panes", "idle")
    expect(heading("run")).toBe("Earlier")
    expect(document.activeElement).toBe(row("run"))
  })

  it("leaves the keyboard where the person put it: nowhere, after a click on text", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    await act(async () => row("run")?.focus())
    await act(async () => {
      clickAway(document.activeElement as HTMLElement)
      await settle(1)
    })
    expect(document.activeElement).toBe(document.body)
    // Another session moves on, and the row's own session changes group: neither takes it back.
    await moveOn(source, "second", "Notarize", "running")
    expect(document.activeElement).toBe(document.body)
    await moveOn(source, "run", "Split panes", "idle")
    expect(heading("run")).toBe("Earlier")
    expect(document.activeElement).toBe(document.body)
  })

  it("keeps it through a press elsewhere that leaves focus where it is", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    await act(async () => row("run")?.focus())
    // The titlebar's drag strip, a scrollbar: pressed, and focus stays put.
    await act(async () => {
      document.body.dispatchEvent(new Event("mousedown", { bubbles: true }))
      await settle(1)
    })
    expect(document.activeElement).toBe(row("run"))
    await moveOn(source, "run", "Split panes", "idle")
    expect(heading("run")).toBe("Earlier")
    expect(document.activeElement).toBe(row("run"))
  })

  it("gives it back when its row goes while a press elsewhere is still held", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    await act(async () => row("run")?.focus())
    // Pressed and held on the drag strip; the press's own task ends.
    await act(async () => {
      document.body.dispatchEvent(new Event("mousedown", { bubbles: true }))
      await new Promise((done) => setTimeout(done, 1))
    })
    // Taken away as Chromium does: told it loses focus, then removed.
    await act(async () => {
      row("run")?.dispatchEvent(new FocusEvent("focusout", { bubbles: true }))
    })
    await moveOn(source, "run", "Split panes", "idle")
    expect(heading("run")).toBe("Earlier")
    expect(document.activeElement).toBe(row("run"))
  })

  it("lets it go on a tap on text, whose focus moves as the finger lifts", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    const before = row("run") as HTMLElement
    await act(async () => before.focus())
    // Contact: a pointer press, in its own task, and nothing moves yet.
    await act(async () => {
      document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }))
      await new Promise((done) => setTimeout(done, 1))
    })
    // Lift: the mouse events a touch sends, and focus moves with them.
    await act(async () => clickAway(before))
    await moveOn(source, "run", "Split panes", "idle")
    expect(heading("run")).toBe("Earlier")
    expect(document.activeElement).toBe(document.body)
  })

  it("leaves it there when a click away and the row's removal land in the same task", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    const before = row("run") as HTMLElement
    await act(async () => before.focus())
    act(() => {
      clickAway(before)
      source.emit({
        kind: "session",
        session: summary("run", "desktop", 600, "idle", {
          title: "Split panes",
          revision: 2,
        }),
      })
    })
    await act(async () => settle(1))
    expect(heading("run")).toBe("Earlier")
    expect(document.activeElement).toBe(document.body)
  })

  it("chooses where focus goes on the frame it lands, after every change before it", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    await act(async () => row("run")?.focus())
    // Its row drawn anew, then its session removed, within one frame.
    await act(async () => {
      source.emit({
        kind: "session",
        session: summary("run", "desktop", 600, "idle", {
          title: "Split panes",
          revision: 2,
        }),
      })
      await settle(1)
      source.emit({ kind: "session-removed", sessionId: "run", revision: 9 })
      await settle(1)
    })
    await nextFrame()
    expect(row("run")).toBeNull()
    expect(document.activeElement).toBe(card("first"))
  })

  it("gives focus back to the reply pill itself when its row is moved, not to the row", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await moveOn(source, "second", "Notarize", "running")
    await open()
    // Beneath its row (no room beside the list): ⌘R, and a reply being typed.
    await press(row("run") as HTMLElement, "KeyR", { command: true })
    const field = host.querySelector<HTMLTextAreaElement>(
      '[data-reply-for="run"] textarea',
    )
    expect(document.activeElement).toBe(field)
    // The row is moved within its list — the element kept — and, as an engine
    // does, focus falls to the page as it goes.
    await act(async () => {
      const item = row("run")?.closest("li") as HTMLElement
      item.parentElement?.insertBefore(item, item.parentElement.firstElementChild)
      field?.blur()
      await settle(1)
    })
    await nextFrame()
    expect(document.activeElement).toBe(field)
  })

  it("falls back to the row when the element it would focus can no longer take focus", async () => {
    await mount()
    await open()
    await act(async () => row("run")?.click())
    await act(async () => settle(10))
    const story = host.querySelector<HTMLElement>(
      ".agents-inline-peek .agents-peek-story",
    )
    await act(async () => story?.focus())
    expect(document.activeElement).toBe(story)
    // No longer focusable as it acts — a button disabled, which a browser
    // will not focus (jsdom would) — and focus falls to the page; the list
    // changes.
    await act(async () => {
      if (!story) return
      story.focus = () => {}
      story.blur()
      story.append(document.createElement("span"))
      await settle(1)
    })
    await nextFrame()
    expect(document.activeElement).toBe(row("run"))
  })

  it("leaves focus alone when the window comes back by a click", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    await act(async () => row("run")?.focus())
    windowFocused = false
    await moveOn(source, "run", "Split panes", "idle")
    windowFocused = true
    // The window returns, and the press that brought it back lands on text.
    await act(async () => {
      window.dispatchEvent(new Event("focus"))
      document.body.dispatchEvent(new Event("mousedown", { bubbles: true }))
      await settle(1)
    })
    await nextFrame()
    expect(document.activeElement).toBe(document.body)
  })

  it("does not take focus back from what took it before the frame", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    await act(async () => row("run")?.focus())
    await act(async () => {
      source.emit({
        kind: "session",
        session: summary("run", "desktop", 600, "idle", {
          title: "Split panes",
          revision: 2,
        }),
      })
      await settle(10)
      // Before the frame the give-back waits for, the keyboard goes elsewhere.
      card("first")?.focus()
    })
    await nextFrame()
    expect(document.activeElement).toBe(card("first"))
  })

  it("gives it back without scrolling the list the person is reading", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    await act(async () => row("run")?.focus())
    for (let frame = 0; frame < 3; frame++) await nextFrame()
    const scrolled = vi.spyOn(HTMLElement.prototype, "scrollIntoView")
    const scrolledTo = vi.spyOn(HTMLElement.prototype, "scrollTo")
    await moveOn(source, "run", "Split panes", "idle")
    for (let frame = 0; frame < 3; frame++) await nextFrame()
    expect(document.activeElement).toBe(row("run"))
    expect(scrolled).not.toHaveBeenCalled()
    expect(scrolledTo).not.toHaveBeenCalled()
  })

  it("keeps it on a row moved within its group", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    // Two working sessions: "second", the later, listed above "run".
    await moveOn(source, "second", "Notarize", "running")
    await open()
    const orderIn = () =>
      [...host.querySelectorAll<HTMLElement>(".agents-row")].map(
        (each) => each.dataset.overviewItem,
      )
    expect(orderIn()).toEqual(["second", "run"])
    const focused = row("second") as HTMLElement
    await act(async () => focused.focus())
    // "run" streams on and is now the latest: the rows swap, and the page
    // moves "second" — the focused row, the same element — below it.
    await act(async () => {
      source.emit({
        kind: "session",
        session: summary("run", "desktop", 5000, "running", {
          title: "Split panes",
          revision: 3,
        }),
      })
      await settle(10)
    })
    expect(orderIn()).toEqual(["run", "second"])
    expect(row("second")).toBe(focused)
    expect(document.activeElement).toBe(focused)
  })

  it("gives it back when the window returns, if it was lost while away", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    await act(async () => row("run")?.focus())
    windowFocused = false
    await moveOn(source, "run", "Split panes", "idle")
    expect(document.activeElement).toBe(document.body)
    windowFocused = true
    await act(async () => {
      window.dispatchEvent(new Event("focus"))
      await settle(1)
    })
    await nextFrame()
    expect(document.activeElement).toBe(row("run"))
  })

  it("follows the peek's own session from beneath its row, when the keyboard chose another", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    // Every session listed, so "run" stays listed once it rests.
    store.dispatch(filterOverview({ filter: { scope: "all", range: "any", tags: [] } }))
    await open()
    await act(async () => row("run")?.click())
    await act(async () => settle(10))
    // The keyboard moves on to another row; the peek stays open beneath "run".
    await press(row("run") as HTMLElement, "ArrowUp")
    expect(document.activeElement).toBe(card("second"))
    const story = host.querySelector<HTMLElement>(
      ".agents-inline-peek .agents-peek-story",
    )
    await act(async () => story?.focus())
    await moveOn(source, "run", "Split panes", "idle")
    expect(heading("run")).not.toBe("Working")
    expect(heading("run")).toBeDefined()
    expect(document.activeElement).toBe(row("run"))
  })

  it("gives back focus whatever took the element away, without a render of its own", async () => {
    await mount()
    await open()
    await act(async () => row("run")?.click())
    await act(async () => settle(10))
    const story = host.querySelector<HTMLElement>(
      ".agents-inline-peek .agents-peek-story",
    )
    await act(async () => story?.focus())
    // Taken off the page by something other than the overview rendering.
    await act(async () => {
      story?.remove()
      await settle(1)
    })
    await nextFrame()
    expect(document.activeElement).toBe(row("run"))
  })

  it("gives back focus Show All had when it goes, once nothing is left out (#317)", async () => {
    const { store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    const showAll = button(host, "Show All")
    if (!showAll) return expect(showAll).toBeDefined()
    await act(async () => showAll.focus())
    await act(async () => showAll.click())
    await nextFrame()
    expect(button(host, "Show All")).toBeUndefined()
    expect(document.activeElement).toBe(card("first"))
  })

  it("leaves focus outside the overview where it is when a row goes", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    const outside = document.createElement("button")
    document.body.append(outside)
    try {
      await act(async () => row("run")?.focus())
      await act(async () => outside.focus())
      await moveOn(source, "run", "Split panes", "idle")
      expect(document.activeElement).toBe(outside)
    } finally {
      outside.remove()
    }
  })

  it("follows its session from the peek beside the list when the window narrows", async () => {
    const observed: ((width: number) => void)[] = []
    class Observer {
      constructor(private readonly callback: ResizeObserverCallback) {}
      observe(target: Element) {
        if (target.classList.contains("workspace-overview-layer"))
          observed.push((width) =>
            this.callback(
              [{ contentRect: { width } } as ResizeObserverEntry],
              this as unknown as ResizeObserver,
            ),
          )
      }
      disconnect() {}
    }
    const was = globalThis.ResizeObserver
    Object.assign(globalThis, { ResizeObserver: Observer })
    try {
      await mount()
      await act(async () => observed.forEach((report) => report(1200)))
      await open()
      await act(async () => row("run")?.click())
      for (
        let wait = 0;
        wait < 5 && !host.querySelector(".agents-overview-peek .agents-peek");
        wait++
      )
        await nextFrame()
      const inPeek = host.querySelector<HTMLElement>(".agents-overview-peek button")
      await act(async () => inPeek?.focus())
      expect(document.activeElement).toBe(inPeek)
      // Narrowed: the peek beside the list goes.
      await act(async () => observed.forEach((report) => report(600)))
      await nextFrame()
      expect(host.querySelector(".agents-overview-peek")).toBeNull()
      expect(document.activeElement).toBe(row("run"))
    } finally {
      Object.assign(globalThis, { ResizeObserver: was })
    }
  })
})

describe("the reply pill keeps the caret", () => {
  it("keeps it with the session when sending moves its row to another group", async () => {
    const { source, store } = await mount()
    store.dispatch(followWorkspace())
    await open()
    // Beneath the row (no room beside the list here): ⌘R, then a reply.
    await press(card("second") as HTMLElement, "KeyR", { command: true })
    const before = host.querySelector<HTMLTextAreaElement>(
      '[data-reply-for="second"] textarea',
    )
    expect(document.activeElement).toBe(before)
    await act(async () => {
      if (!before) return
      const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")
      set?.set?.call(before, "Use the beta profile.")
      before.dispatchEvent(new Event("input", { bubbles: true }))
    })
    await press(before as HTMLElement, "Enter")
    // The source takes it, lets the approval go, and says the session works
    // on: its row moves from Needs you to Working, its pill drawn anew.
    await act(async () => {
      const held = source.transcripts.get("second")
      if (held) {
        const next = { ...held, approval: null, revision: held.revision + 1 }
        source.transcripts.set("second", next)
        source.emit({ kind: "transcript", transcript: next })
      }
      source.emit({
        kind: "session",
        session: summary("second", "desktop", 400, "running", {
          title: "Notarize",
          revision: 2,
        }),
      })
      await settle(10)
    })
    await nextFrame()
    expect(card("second")).toBeNull()
    expect(row("second")).not.toBeNull()
    const after = host.querySelector<HTMLTextAreaElement>(
      '[data-reply-for="second"] textarea',
    )
    expect(after).not.toBeNull()
    expect(after).not.toBe(before)
    expect(document.activeElement).toBe(after)
  })

  it("lets the caret go once the person moves it: a pill drawn again later does not take it", async () => {
    await mount()
    await open()
    await press(card("second") as HTMLElement, "KeyR", { command: true })
    const field = host.querySelector<HTMLTextAreaElement>(
      '[data-reply-for="second"] textarea',
    )
    expect(document.activeElement).toBe(field)
    // Escape: back to the row; then the row is clicked twice, closing and
    // opening its peek again.
    await press(field as HTMLElement, "Escape")
    expect(document.activeElement).toBe(card("second"))
    await act(async () => card("second")?.click())
    await act(async () => card("second")?.click())
    expect(host.querySelector('[data-reply-for="second"] textarea')).not.toBeNull()
    expect(document.activeElement).toBe(card("second"))
  })
})

describe("Escape in the overview", () => {
  it("leaves Escape in a menu over it to the menu", async () => {
    const { store } = await mount()
    await open()
    const menu = document.createElement("div")
    menu.setAttribute("role", "menu")
    const item = document.createElement("div")
    item.setAttribute("role", "menuitem")
    menu.append(item)
    document.body.append(menu)
    await press(item, "Escape")
    expect(selectOverviewOpen(store.getState())).toBe(true)
    menu.remove()
  })
})

describe("the frame the overview opens on", () => {
  // Frames come when the test says (`nextFrame`), so the opening frame is one frame.

  it("leaves at once, before the keyboard has landed on its row", async () => {
    const { store } = await mount()
    const composer = host.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message"]',
    )
    composer?.focus()
    // ⌘0's dispatch, and Escape straight after it: no frame has passed.
    await act(async () => store.dispatch(showContent({ content: "agents" })))
    expect(selectOverviewOpen(store.getState())).toBe(true)
    expect(document.activeElement).toBe(composer)
    await press(composer as HTMLElement, "Escape")
    expect(selectOverviewOpen(store.getState())).toBe(false)
  })

  it("lays the page out once: the keyboard arrives a frame later, on what is already current", async () => {
    await mount()
    const focus = HTMLElement.prototype.focus
    const early: Element[] = []
    HTMLElement.prototype.focus = function (this: HTMLElement, options) {
      if (this.closest(".agents-overview")) early.push(this)
      return focus.call(this, options)
    }
    try {
      await act(async () => entry()?.click())
      await act(async () => settle(10))
    } finally {
      HTMLElement.prototype.focus = focus
    }
    // Focusing in the opening frame would make it lay the page out early.
    expect(early).toEqual([])
    await nextFrame()
    await nextFrame()
    expect(document.activeElement).toBe(card("first"))
  })

  it("opens in the arrangement its layer measured before it opened, and draws the peek beside the list a frame on", async () => {
    // The layer is on the page, 1,200px wide, before the overview opens: wide enough for the peek beside it.
    const observed: ((width: number) => void)[] = []
    class Observer {
      constructor(private readonly callback: ResizeObserverCallback) {}
      observe(target: Element) {
        if (target.classList.contains("workspace-overview-layer"))
          observed.push((width) =>
            this.callback(
              [{ contentRect: { width } } as ResizeObserverEntry],
              this as unknown as ResizeObserver,
            ),
          )
      }
      disconnect() {}
    }
    const was = globalThis.ResizeObserver
    Object.assign(globalThis, { ResizeObserver: Observer })
    try {
      await mount()
      await act(async () => observed.forEach((report) => report(1200)))
      await act(async () => entry()?.click())
      expect(host.querySelector(".agents-overview")?.hasAttribute("data-split")).toBe(
        true,
      )
      expect(host.querySelector(".agents-overview-peek")?.childElementCount).toBe(0)
      // A frame on, the peek is drawn.
      for (let wait = 0; wait < 5 && !host.querySelector(".agents-peek"); wait++)
        await nextFrame()
      expect(host.querySelector(".agents-overview-peek .agents-peek")).not.toBeNull()
    } finally {
      Object.assign(globalThis, { ResizeObserver: was })
    }
  })
})

it("asks in its peek with the pane's own approval parts, one component for both", async () => {
  await mount()
  await open()
  await act(async () => card("first")?.click())
  await act(async () => settle(10))
  const peek = host.querySelector(".agents-inline-peek")
  expect(peek?.querySelector(".workspace-approval-command")?.textContent).toBe(
    "$ security import build.p12",
  )
  expect(peek?.querySelector(".workspace-approval-actions")).not.toBeNull()
})
