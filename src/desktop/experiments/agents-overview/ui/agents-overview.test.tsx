// @vitest-environment jsdom
/**
 * The agents overview in a real desktop store over a fake source: absent
 * until the experiment is on; each waiting approval shown with its command
 * and answered in place from the keyboard, as the person, the keyboard moving
 * on to the next; a refused answer said, and answerable again; and opening a
 * session putting the overview away.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { ClockProvider, loadWorkspace, setComposerText } from "../../../workspace"
import { fakeSource, settle, summary, testStore } from "../../../workspace/testing"
import type { WorkspaceIndex } from "../../../workspace/model/workspace-index"
import { selectFocusedSessionId } from "../adapters/workspace-bridge"
import {
  AgentsOverviewArea,
  AgentsOverviewEntry,
  AgentsOverviewScope,
} from "./overview-scope"

/**
 * Turns the experiment on or off as Settings does, by the window's own change
 * event — which applies whether or not storage keeps it.
 */
async function turn(experiment: "on" | "off") {
  await act(async () => {
    window.dispatchEvent(
      new CustomEvent("nessa:desktop-experiments-agents-overview", {
        detail: experiment,
      }),
    )
  })
}

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

async function mount(experiment: "on" | "off" = "on") {
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
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  await act(async () =>
    root.render(
      <Provider store={store}>
        <ClockProvider now={() => 1000}>
          <AgentsOverviewScope>
            <nav>
              <AgentsOverviewEntry />
            </nav>
            <AgentsOverviewArea>
              <main className="workspace-chat">
                <textarea aria-label="Reply" />
              </main>
            </AgentsOverviewArea>
          </AgentsOverviewScope>
        </ClockProvider>
      </Provider>,
    ),
  )
  await turn(experiment)
  await act(async () => settle(10))
  return { source, store }
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

/** Opens the overview, and waits out its first frames, after which the keyboard is on it. */
async function open() {
  await act(async () => entry()?.click())
  await act(async () => settle(10))
  for (let frame = 0; frame < 2; frame++)
    await act(
      async () => new Promise<void>((done) => requestAnimationFrame(() => done())),
    )
}

/** ⌘↩ and the like, as a keyboard off the Mac sends them: Control stands for ⌘. */
async function press(
  target: HTMLElement,
  code: string,
  modifiers: { command?: boolean; alt?: boolean } = {},
) {
  await act(async () => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", {
        code,
        key: code,
        ctrlKey: modifiers.command ?? false,
        altKey: modifiers.alt ?? false,
        bubbles: true,
        cancelable: true,
      }),
    )
    await settle(10)
  })
}

describe("the agents overview", () => {
  it("offers nothing while the experiment is off", async () => {
    await mount("off")
    expect(entry()).toBeUndefined()
    expect(host.querySelector(".agents-overview")).toBeNull()
  })

  it("follows the experiment being turned off from Settings, in the same window", async () => {
    await mount("on")
    await open()
    expect(host.querySelector(".agents-overview")).not.toBeNull()
    await turn("off")
    expect(entry()).toBeUndefined()
    expect(host.querySelector(".agents-overview")).toBeNull()
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
    // The panes are kept, inert, under the overview.
    expect(host.querySelector(".agents-overview-under")?.hasAttribute("inert")).toBe(true)
    expect(host.querySelector("textarea")).not.toBeNull()
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
    await press(card("second") as HTMLElement, "Backspace", { command: true })
    expect(source.calls).toContainEqual(["deny", "second", "second-ask", "person"])
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

  it("says why an answer was refused, and lets the person answer again", async () => {
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

  it("goes back to the workspace on Escape", async () => {
    await mount()
    await open()
    await press(card("first") as HTMLElement, "Escape")
    expect(host.querySelector(".agents-overview")).toBeNull()
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

  it("lists every session once All is chosen, and counts what it lists", async () => {
    await mount()
    await open()
    await act(async () => {
      window.dispatchEvent(
        new CustomEvent("nessa:desktop-experiments-agents-overview-filter", {
          detail: JSON.stringify({ scope: "all", range: "any", tags: [] }),
        }),
      )
    })
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
    // Emptied, and still where the person is writing.
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

  it("opens in the arrangement its place allows, and draws the peek beside the list a frame on", async () => {
    // The chat area was 1,200px wide before the overview came: wide enough for the peek beside it.
    const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "offsetWidth")
    Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
      configurable: true,
      get: () => 1200,
    })
    try {
      await mount()
      await act(async () => entry()?.click())
      expect(host.querySelector(".agents-overview")?.hasAttribute("data-split")).toBe(
        true,
      )
      expect(host.querySelector(".agents-overview-peek")?.childElementCount).toBe(0)
      // A frame on, and once its conversation is read, the peek is drawn.
      for (let wait = 0; wait < 5 && !host.querySelector(".agents-peek"); wait++)
        await frame()
      expect(host.querySelector(".agents-overview-peek .agents-peek")).not.toBeNull()
    } finally {
      if (width) Object.defineProperty(HTMLElement.prototype, "offsetWidth", width)
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
