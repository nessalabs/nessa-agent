// @vitest-environment jsdom
/**
 * Widgets in the workspace (ADR 326), through the whole shell and the sample
 * plugin: a card in a message opening a pane beside its conversation, the
 * widget pane's chrome, the window over the panes and the ways it is left,
 * Escape's order, and where focus lands.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { isMac } from "../../../adapters/platform"
import { chordEvent } from "../../../model/keyboard"
import { panesOf } from "../../../split-panes/model/pane-layout"
import {
  createWidgetRegistry,
  samplePlugin,
  sampleWidgets,
  WidgetRegistryProvider,
  type WidgetPlugin,
  type WidgetRef,
} from "../../../widgets"
import { ClockProvider } from "../../adapters/dom/clock"
import {
  closePane,
  focusPane,
  loadWorkspace,
  openBeside,
  openSession,
  openWidget,
} from "../../adapters/store/commands"
import { emptyTranscript } from "../../model/transcript"
import {
  controlledAnimationFrames,
  fakeSource,
  flushAnimationFrames,
  settle,
  shownIn,
  testStore,
  type AnimationFrames,
} from "../../testing"
import { workspaceShortcuts } from "./shortcuts"
import { ThreeColumns } from "./three-columns"

let host: HTMLDivElement
let root: Root
let animation: AnimationFrames

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  animation = controlledAnimationFrames()
  class Observer {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  Object.assign(globalThis, { ResizeObserver: Observer, IntersectionObserver: Observer })
  Element.prototype.scrollTo ??= () => {}
  Element.prototype.scrollIntoView ??= () => {}
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
  })) as unknown as typeof window.matchMedia
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
  animation.restore()
})

type Store = ReturnType<typeof testStore>

/** The shell over session a — whose conversation carries the sample trail's card — with the sample plugin registered. */
async function render(): Promise<Store> {
  const source = fakeSource()
  source.transcripts.set("a", {
    ...emptyTranscript("a"),
    messages: [
      {
        id: "a-1",
        role: "agent",
        at: 900,
        parts: [
          { kind: "text", text: "The trail:" },
          { kind: "widget", widget: sampleWidgets.trail },
        ],
      },
    ],
    revision: 1,
  })
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  await settle()
  store.dispatch(openSession({ sessionId: "a" }))
  await settle()
  const registry = createWidgetRegistry<WidgetPlugin>([samplePlugin("a")])
  await act(async () =>
    root.render(
      <Provider store={store}>
        <WidgetRegistryProvider registry={registry}>
          <ClockProvider now={() => 1000}>
            <ThreeColumns hostKind="browser" browserSurface />
          </ClockProvider>
        </WidgetRegistryProvider>
      </Provider>,
    ),
  )
  await frames()
  return store
}

/** The frames focus asked for, run because the test says so. */
const frames = () => flushAnimationFrames(animation, act)
const shown = (store: Store) => shownIn(store.getState().workspace.panes)
const content = (store: Store) => store.getState().workspace.content
/** `value`, or a failed test saying what was not there. */
function present<T>(value: T | null | undefined, what: string): T {
  if (value === null || value === undefined) throw new Error(`no ${what}`)
  return value
}
const windowShown = () => host.querySelector("[data-widget-window]")
const theWindow = () => present(windowShown(), "window")
const layout = (store: Store) => present(store.getState().workspace.panes, "panes")
const firstPane = (store: Store) => panesOf(layout(store))[0].key
const caret = () => document.activeElement

const press = (command: string) => {
  const binding = workspaceShortcuts.find((each) => each.command === command)
  if (!binding) throw new Error(`no ${command}`)
  window.dispatchEvent(
    new KeyboardEvent("keydown", { ...chordEvent(binding.chord, isMac), bubbles: true }),
  )
}
/** Escape where focus is, as the browser sends it: from the focused element up. */
const escape = async (from: Element | null = document.activeElement) => {
  await act(async () =>
    (from ?? document.body).dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        code: "Escape",
        bubbles: true,
        cancelable: true,
      }),
    ),
  )
  await frames()
}
const click = async (element: Element | null | undefined) => {
  if (!element) throw new Error("nothing to click")
  await act(async () => (element as HTMLElement).click())
  await frames()
}
const button = (scope: ParentNode | null | undefined, name: string) =>
  [...present(scope, `place for ${name}`).querySelectorAll("button")].find(
    (each) =>
      each.textContent === name || each.getAttribute("aria-label")?.startsWith(name),
  )
const paneOf = (widget: WidgetRef) =>
  present(
    [...host.querySelectorAll<HTMLElement>("[data-widget-pane]")].find((pane) =>
      pane.querySelector(`[data-sample-view="${widget.id}"]`),
    ),
    `pane of ${widget.id}`,
  )
const bodyOf = (scope: Element | null | undefined) =>
  scope?.querySelector<HTMLElement>("[data-widget-body]") ?? null

async function openTrailBeside(store: Store) {
  await click(button(host.querySelector("[data-sample-card]"), "Open"))
  expect(shown(store)).toEqual(["a", "widget sample/trail"])
  return paneOf(sampleWidgets.trail)
}

describe("a card in a message", () => {
  it("opens its widget in a pane beside the conversation, and the caret goes into its body", async () => {
    const store = await render()
    const pane = await openTrailBeside(store)
    expect(pane.querySelector("nav")?.textContent).toBe("Session aSample trail")
    expect(caret()).toBe(bodyOf(pane))
  })
})

describe("the widget pane", () => {
  it("opens a pane whose body is the widget", async () => {
    const store = await render()
    await act(async () => {
      void store.dispatch(
        openWidget({ widget: sampleWidgets.trail, place: "pane", origin: "a" }),
      )
    })
    const pane = host.querySelector("[data-widget-pane]")
    expect(pane?.querySelector("nav")?.textContent).toBe("Session aSample trail")
    await frames()
    expect(pane?.querySelector("[data-widget-body]")).not.toBeNull()
  })

  it("goes back to its conversation: focusing the pane showing it, or opening it beside", async () => {
    const store = await render()
    const pane = await openTrailBeside(store)
    await click(button(pane.querySelector("nav"), "Session a"))
    expect(shown(store)).toEqual(["a", "widget sample/trail"])
    expect(firstPane(store)).toBe(layout(store).focused)
    // With the conversation gone from the panes, it opens beside.
    store.dispatch(
      openSession({
        sessionId: "b",
        pane: firstPane(store),
      }),
    )
    await frames()
    await click(button(paneOf(sampleWidgets.trail).querySelector("nav"), "Session a"))
    expect(shown(store)).toContain("a")
    expect(shown(store)).toContain("widget sample/trail")
  })

  it("closes as a pane does, the caret going to the pane that takes focus", async () => {
    const store = await render()
    const pane = await openTrailBeside(store)
    await click(button(pane, "Close Pane"))
    expect(shown(store)).toEqual(["a"])
    expect(caret()?.tagName).toBe("TEXTAREA")
  })

  it("puts the caret in its body when ⌘1–4 or ⌘W lands on it", async () => {
    const store = await render()
    await openTrailBeside(store)
    await act(async () => press("focusPane1"))
    await frames()
    expect(caret()?.tagName).toBe("TEXTAREA")
    await act(async () => press("focusPane2"))
    await frames()
    expect(caret()).toBe(bodyOf(paneOf(sampleWidgets.trail)))
    // ⌘W on a, the other pane: focus lands on the widget pane, in its body.
    await act(async () => press("focusPane1"))
    await frames()
    await act(async () => press("closePane"))
    await frames()
    expect(shown(store)).toEqual(["widget sample/trail"])
    expect(caret()).toBe(bodyOf(paneOf(sampleWidgets.trail)))
  })

  it("draws a line with close for a widget it cannot show", async () => {
    const store = await render()
    for (const [widget, said] of [
      [sampleWidgets.off, "Turned off in Settings › Advanced › Experimental"],
      [sampleWidgets.missing, "This is no longer available"],
    ] as const) {
      await act(
        async () =>
          void store.dispatch(openWidget({ widget, place: "pane", origin: "a" })),
      )
      await frames()
      const pane = [...host.querySelectorAll("[data-widget-pane]")].find((each) =>
        each.textContent?.includes(said),
      )
      expect(pane, said).toBeDefined()
      expect(pane?.querySelector("nav")?.textContent).toBe("Sample")
      await click(button(present(pane, said).querySelector(".widget-body"), "Close"))
      expect(shown(store)).toEqual(["a"])
    }
  })

  it("keeps a pane's Escape to the view's steps back while focus is inside it, and never closes it", async () => {
    const store = await render()
    const pane = await openTrailBeside(store)
    await click(pane.querySelector('[data-sample-step="1"]'))
    expect(pane.querySelector("[data-sample-detail]")).not.toBeNull()
    expect(caret()?.hasAttribute("data-sample-back")).toBe(true)
    await escape()
    expect(pane.querySelector("[data-sample-detail]")).toBeNull()
    // The detail's Back held the caret; gone, the caret falls back to the widget's body.
    expect(caret()).toBe(bodyOf(pane))
    await escape()
    expect(shown(store)).toEqual(["a", "widget sample/trail"])
    // With focus outside the pane, its steps back are not Escape's.
    await click(pane.querySelector('[data-sample-step="0"]'))
    await act(async () => store.dispatch(focusPane({ pane: firstPane(store) })))
    await frames()
    await escape()
    expect(pane.querySelector("[data-sample-detail]")).not.toBeNull()
  })
})

describe("the session's header", () => {
  it("draws each plugin's accessory for its session, which opens beside that session", async () => {
    const store = await render()
    const accessory = host.querySelector("[data-sample-accessory]")
    expect(accessory?.textContent).toBe("Notes")
    await click(accessory)
    expect(shown(store)).toEqual(["a", "widget sample/notes"])
  })

  it("adds nothing to the header of a session a plugin has nothing for", async () => {
    const store = await render()
    await act(async () => void store.dispatch(openSession({ sessionId: "b" })))
    await frames()
    const header = present(host.querySelector("[data-pane-focused] header"), "header")
    // Straight from the spacer to the pane's actions, as with no plugin at all.
    expect(header.querySelector(".workspace-spacer")?.nextElementSibling?.className).toBe(
      "workspace-pane-actions",
    )
  })
})

describe("the window", () => {
  it("opens its body with the window", async () => {
    const store = await render()
    await act(async () => {
      void store.dispatch(
        openWidget({ widget: sampleWidgets.trail, place: "window", origin: "a" }),
      )
    })
    expect(windowShown()?.querySelector("[data-widget-body]")).not.toBeNull()
  })

  async function openWindow(store: Store) {
    const pane = await openTrailBeside(store)
    await click(button(pane, "Open in Window"))
    expect(content(store)).toEqual({ widget: sampleWidgets.trail })
    expect(host.querySelector(".workspace")?.getAttribute("data-content")).toBe("widget")
    expect(caret()).toBe(bodyOf(windowShown()))
    return pane
  }

  it("is left by Escape and by its close, back to the panes as they were, the caret in the focused pane", async () => {
    const store = await render()
    await openWindow(store)
    await escape()
    expect(content(store)).toBe("panes")
    expect(windowShown()).toBeNull()
    expect(shown(store)).toEqual(["a", "widget sample/trail"])
    expect(caret()).toBe(bodyOf(paneOf(sampleWidgets.trail)))
    await click(button(paneOf(sampleWidgets.trail), "Open in Window"))
    await click(button(theWindow(), "Close"))
    expect(content(store)).toBe("panes")
  })

  it("is closed by ⌘W, never a pane beneath it; left for a session chosen; and ⌘0 goes to the overview", async () => {
    const store = await render()
    await openWindow(store)
    await act(async () => press("closePane"))
    await frames()
    expect(content(store)).toBe("panes")
    expect(shown(store)).toEqual(["a", "widget sample/trail"])
    await click(button(paneOf(sampleWidgets.trail), "Open in Window"))
    await act(async () => void store.dispatch(openBeside({ sessionId: "b" })))
    await frames()
    expect(content(store)).toBe("panes")
    await click(button(paneOf(sampleWidgets.trail), "Open in Window"))
    await act(async () => press("showOverview"))
    await frames()
    expect(content(store)).toBe("agents")
  })

  it("goes back to its conversation by its trail, focusing the pane showing it", async () => {
    const store = await render()
    await openWindow(store)
    await click(button(theWindow().querySelector("nav"), "Session a"))
    expect(content(store)).toBe("panes")
    expect(firstPane(store)).toBe(layout(store).focused)
    expect(caret()?.tagName).toBe("TEXTAREA")
  })

  it("steps back in its view first: Escape closes the detail, the caret stays in the window, and the next Escape leaves", async () => {
    const store = await render()
    await openWindow(store)
    await click(theWindow().querySelector('[data-sample-step="2"]'))
    await escape()
    expect(content(store)).toEqual({ widget: sampleWidgets.trail })
    expect(theWindow().querySelector("[data-sample-detail]")).toBeNull()
    expect(caret()).toBe(bodyOf(windowShown()))
    await escape()
    expect(content(store)).toBe("panes")
  })

  it("is left by Escape from a session row, and from the search only once its query is cleared", async () => {
    const store = await render()
    await openWindow(store)
    const row = host.querySelector<HTMLElement>('.workspace-list [data-session-row="b"]')
    expect(row).not.toBeNull()
    present(row, "row").focus()
    await escape(row)
    expect(content(store)).toBe("panes")
    await click(button(paneOf(sampleWidgets.trail), "Open in Window"))
    const search = present(
      host.querySelector<HTMLInputElement>(".workspace-search input"),
      "search",
    )
    search.focus()
    await act(async () => {
      // As typing does: through the input's own setter, which React hears.
      const set = present(
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set,
        "value setter",
      )
      set.call(search, "Session")
      search.dispatchEvent(new Event("input", { bubbles: true }))
    })
    await escape(search)
    expect(search.value).toBe("")
    expect(content(store)).toEqual({ widget: sampleWidgets.trail })
    await escape(search)
    expect(content(store)).toBe("panes")
  })

  it("leaves Escape to a menu open over it", async () => {
    const store = await render()
    await openWindow(store)
    const menu = document.createElement("div")
    menu.setAttribute("role", "menu")
    const item = document.createElement("button")
    menu.append(item)
    theWindow().append(menu)
    await escape(item)
    expect(content(store)).toEqual({ widget: sampleWidgets.trail })
    menu.remove()
  })

  it("replaces its widget with another opened there, drawn afresh: no step back of the last one's, the caret in its body", async () => {
    const store = await render()
    await openWindow(store)
    await click(theWindow().querySelector('[data-sample-step="2"]'))
    const show = async (widget: WidgetRef) => {
      await act(async () => void store.dispatch(openWidget({ widget, place: "window" })))
      await frames()
    }
    await show(sampleWidgets.notes)
    expect(content(store)).toEqual({ widget: sampleWidgets.notes })
    expect(theWindow().querySelector('[data-sample-view="notes"]')).not.toBeNull()
    expect(caret()).toBe(bodyOf(windowShown()))
    await show(sampleWidgets.trail)
    expect(theWindow().querySelector("[data-sample-detail]")).toBeNull()
    // No step back left over: the first Escape is the host's.
    await escape()
    expect(content(store)).toBe("panes")
  })
})

describe("a widget pane showing another widget in its place", () => {
  it("draws it afresh: no view state, no step back of the last one's", async () => {
    const store = await render()
    await openTrailBeside(store)
    // The conversation closed, the trail's pane is the focused pane's place.
    await act(async () => void store.dispatch(closePane({ pane: firstPane(store) })))
    await frames()
    expect(shown(store)).toEqual(["widget sample/trail"])
    const pane = paneOf(sampleWidgets.trail)
    await click(pane.querySelector('[data-sample-step="2"]'))
    const show = async (widget: WidgetRef) => {
      await act(
        async () =>
          void store.dispatch(openWidget({ widget, place: "pane", origin: "a" })),
      )
      await frames()
    }
    await show(sampleWidgets.notes)
    expect(shown(store)).toEqual(["widget sample/notes"])
    // Escape in the notes is no one's: the trail's step back went with it.
    const body = bodyOf(paneOf(sampleWidgets.notes))
    body?.focus()
    const event = new KeyboardEvent("keydown", {
      key: "Escape",
      bubbles: true,
      cancelable: true,
    })
    await act(async () => void body?.dispatchEvent(event))
    expect(event.defaultPrevented).toBe(false)
    await show(sampleWidgets.trail)
    expect(paneOf(sampleWidgets.trail).querySelector("[data-sample-detail]")).toBeNull()
  })
})
