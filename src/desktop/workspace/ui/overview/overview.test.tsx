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
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { ClockProvider } from "../../adapters/dom/clock"
import { focusedPaneAttribute } from "../../adapters/dom/focus"
import {
  approve,
  filterOverview,
  loadWorkspace,
  setComposerText,
  showContent,
} from "../../adapters/store/commands"
import { selectFocusedSessionId } from "../../adapters/store/selectors"
import { fakeSource, keptFilter, settle, summary, testStore } from "../../testing"
import type { WorkspaceIndex } from "../../model/workspace-index"
import { OverviewRow } from "../source-list/overview-row"
import { answerPause } from "../../model/overview/walk"
import { OverviewLayer } from "./overview-layer"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  Element.prototype.scrollIntoView ??= () => {}
  Element.prototype.scrollTo ??= () => {}
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
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
      summary("run", "desktop", 250, "running", { title: "Split panes" }),
      summary("rest", "desktop", 100),
    ],
  }
}

async function mount({
  strict = false,
  wrap = (tree: ReactNode) => tree,
}: { strict?: boolean; wrap?: (tree: ReactNode) => ReactNode } = {}) {
  const source = fakeSource(sampleIndex())
  for (const [sessionId, command] of [
    ["first", "security import build.p12"],
    ["second", "xcrun notarytool submit build.dmg"],
  ] as const) {
    const held = source.transcripts.get(sessionId)
    if (held)
      source.transcripts.set(sessionId, {
        ...held,
        approval: { id: `${sessionId}-ask`, command, reason: `Why ${sessionId}.` },
      })
  }
  const running = source.transcripts.get("run")
  if (running)
    source.transcripts.set("run", {
      ...running,
      messages: [
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

/** Waits out a frame. */
const nextFrame = () =>
  act(async () => new Promise<void>((done) => requestAnimationFrame(() => done())))

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

describe("the agents overview", () => {
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
    ])
    expect(document.activeElement).toBe(card("second"))
    // Held in place while it settles, saying what became of it.
    expect(card("first")?.dataset.phase).toBe("settled")
    // A new press, once the person can see where the keyboard went.
    await act(async () => new Promise((done) => setTimeout(done, answerPause)))
    await press(card("second") as HTMLElement, "Backspace", { command: true })
    expect(source.calls).toContainEqual(["deny", "second", "second-ask", "person"])
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
    await act(async () => new Promise((done) => setTimeout(done, 80)))
    await press(document.activeElement as HTMLElement, "Enter", { command: true })
    // Nor does Return open it.
    await press(document.activeElement as HTMLElement, "Enter")
    const answers = source.calls.filter(
      (call) => call[0] === "approve" || call[0] === "deny",
    )
    expect(answers).toEqual([["approve", "first", "first-ask", "once", "person"]])
    expect(document.activeElement).toBe(card("second"))
    expect(host.querySelector('[data-content="panes"]')).toBeNull()
  })

  it("gives the region back when the preview is turned off, in this window or another", async () => {
    const { store } = await mount()
    // Turned on in this window, as Settings does.
    await act(async () => {
      window.dispatchEvent(
        new CustomEvent("nessa:desktop-experiments-agents-overview", { detail: "on" }),
      )
    })
    await open()
    expect(store.getState().workspace.content).toBe("agents")
    // Another window beside this one turns it off: the storage event says so
    // (nothing is stored here, which reads as off).
    await act(async () => {
      window.dispatchEvent(
        new StorageEvent("storage", { key: "nessa.desktop.experiments.agents-overview" }),
      )
    })
    expect(store.getState().workspace.content).toBe("panes")
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
    ])
  })

  it("shares one answer with a pane: an answer on its way from anywhere rests its buttons", async () => {
    const { source, store } = await mount()
    await open()
    source.hold("approve")
    // As the pane's card would, or an agent.
    const fromPane = store.dispatch(
      approve({ sessionId: "first", approvalId: "first-ask", initiator: "agent" }),
    )
    await act(async () => settle(10))
    expect(button(card("first"), "Allow")?.disabled).toBe(true)
    await press(card("first") as HTMLElement, "Enter", { command: true })
    expect(source.calls.filter((call) => call[0] === "approve")).toHaveLength(1)
    await source.release("approve")
    expect(await fromPane).toBe("sent")
  })

  it("says why an answer was not confirmed, and lets the person answer again", async () => {
    const { source } = await mount()
    source.refuse("approve", "unavailable")
    await open()
    await press(card("first") as HTMLElement, "Enter", { command: true })
    expect(card("first")?.dataset.phase).toBeUndefined()
    expect(card("first")?.querySelector(".agents-request-failure")?.textContent).toBe(
      "Nessa couldn’t reach your sessions.",
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
    ])
  })

  it("rests on the list once the last request is answered, rather than jumping away", async () => {
    await mount()
    await open()
    await press(card("first") as HTMLElement, "Enter", { command: true })
    await act(async () => new Promise((done) => setTimeout(done, answerPause)))
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
    await act(
      async () => new Promise<void>((done) => requestAnimationFrame(() => done())),
    )
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

describe("the frame the overview opens on", () => {
  // Frames come when the test says, so "the opening frame" is one frame.
  let queued: FrameRequestCallback[] = []
  const request = window.requestAnimationFrame
  const cancel = window.cancelAnimationFrame
  beforeEach(() => {
    queued = []
    window.requestAnimationFrame = (callback) => queued.push(callback)
    window.cancelAnimationFrame = (id) => {
      queued[id - 1] = () => {}
    }
  })
  afterEach(() => {
    window.requestAnimationFrame = request
    window.cancelAnimationFrame = cancel
  })
  const frame = () =>
    act(async () => {
      const due = queued
      queued = []
      due.forEach((run) => run(0))
      await settle(10)
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
    await frame()
    await frame()
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
        await frame()
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
