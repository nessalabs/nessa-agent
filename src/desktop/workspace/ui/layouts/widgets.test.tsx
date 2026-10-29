// @vitest-environment jsdom
/**
 * Widgets in the workspace (ADR 326), through the whole shell and the sample
 * plugin: a card in a message opening a tab in its chat's right panel, the
 * panel's tabs and chrome, the window over the panes and the ways it is
 * left, Escape's order, and where focus lands.
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
import { ClockProvider } from "../../../../clock"
import {
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
const panel = () => present(host.querySelector<HTMLElement>("#workspace-panel"), "panel")
/** The panel's tab in front: its widget's part of the panel. */
const inFront = () =>
  present(
    panel().querySelector<HTMLElement>("[data-panel-widget]"),
    "panel's tab in front",
  )
const tabTitles = () =>
  [...panel().querySelectorAll('.workspace-panel-tabs [role="tab"]')].map((tab) =>
    tab.getAttribute("title"),
  )
const panelOf = (store: Store, sessionId: string) =>
  store.getState().workspace.panels[sessionId]
const bodyOf = (scope: Element | null | undefined) =>
  scope?.querySelector<HTMLElement>("[data-widget-body]") ?? null

async function openTrail(store: Store) {
  await click(button(host.querySelector("[data-sample-card]"), "Open"))
  expect(shown(store)).toEqual(["a"])
  expect(panelOf(store, "a")?.front).toEqual(sampleWidgets.trail)
  return inFront()
}

describe("a card in a message", () => {
  it("opens its widget as a tab in its chat's panel, never a pane, and the caret goes into its body", async () => {
    const store = await render()
    const shownPanel = await openTrail(store)
    expect(panel().hasAttribute("inert")).toBe(false)
    expect(tabTitles()).toEqual(["Sample trail"])
    expect(shownPanel.querySelector("[data-sample-view]")).not.toBeNull()
    // Its chat is the one the panel is for: no way back to it is drawn.
    expect(shownPanel.querySelector("nav")?.textContent).toBe("Sample trail")
    expect(caret()).toBe(bodyOf(shownPanel))
    // The panel's widget tabs are distinct from the window's conversation tabs.
    expect(
      [...host.querySelectorAll('[role="tablist"]')].filter(
        (list) => !list.closest("#workspace-panel"),
      ),
    ).toHaveLength(1)
  })
})

describe("the chat's panel", () => {
  it("holds each widget opened for the chat as a tab, the last in front, each once", async () => {
    const store = await render()
    await openTrail(store)
    await click(host.querySelector("[data-sample-accessory]"))
    expect(tabTitles()).toEqual(["Sample trail", "Sample notes"])
    expect(inFront().querySelector('[data-sample-view="notes"]')).not.toBeNull()
    await click(button(host.querySelector("[data-sample-card]"), "Open"))
    expect(tabTitles()).toEqual(["Sample trail", "Sample notes"])
    expect(inFront().querySelector('[data-sample-view="trail"]')).not.toBeNull()
  })

  it("brings a tab to the front when it is clicked, drawn afresh: no step back of the last one's", async () => {
    const store = await render()
    await openTrail(store)
    await click(inFront().querySelector('[data-sample-step="2"]'))
    await click(host.querySelector("[data-sample-accessory]"))
    const tab = (title: string) =>
      present(
        panel().querySelector<HTMLElement>(
          `.workspace-panel-tabs [role="tab"][title="${title}"]`,
        ),
        `tab ${title}`,
      )
    await click(tab("Sample trail"))
    expect(panelOf(store, "a")?.front).toEqual(sampleWidgets.trail)
    expect(inFront().querySelector("[data-sample-detail]")).toBeNull()
  })

  it("closes a tab by its close, the next coming to the front, and says so once none is left", async () => {
    const store = await render()
    await openTrail(store)
    await click(host.querySelector("[data-sample-accessory]"))
    await click(
      panel().querySelector(
        '[aria-label^="Close Sample notes"], [title^="Close Sample notes"]',
      ),
    )
    expect(tabTitles()).toEqual(["Sample trail"])
    await click(panel().querySelector('[title^="Close Sample trail"]'))
    expect(panelOf(store, "a")).toBeUndefined()
    expect(panel().textContent).toBe("Nothing open")
    // The panel stays open, as the person left it.
    expect(panel().hasAttribute("inert")).toBe(false)
  })

  it("keeps the caret in the panel as a tab closes, and gives it to the chat once none is left", async () => {
    const store = await render()
    await openTrail(store)
    await click(host.querySelector("[data-sample-accessory]"))
    const closeOf = (title: string) =>
      present(
        panel().querySelector<HTMLElement>(`[title^="Close ${title}"]`),
        `close of ${title}`,
      )
    // Focus on a tab's close falls away with it: into the tab now in front.
    await act(async () => closeOf("Sample notes").focus())
    await click(closeOf("Sample notes"))
    expect(caret()).toBe(bodyOf(inFront()))
    // The last one gone, into the focused chat's composer.
    await act(async () => closeOf("Sample trail").focus())
    await click(closeOf("Sample trail"))
    expect(caret()).toBe(host.querySelector(".desktop-composer textarea"))
  })

  it("is each chat's own: another chat in front shows its panel, and the first one's comes back with it", async () => {
    const store = await render()
    await openTrail(store)
    await act(async () => void store.dispatch(openSession({ sessionId: "b" })))
    await frames()
    expect(panel().textContent).toBe("Nothing open")
    await act(async () => void store.dispatch(openSession({ sessionId: "a" })))
    await frames()
    expect(tabTitles()).toEqual(["Sample trail"])
  })

  it("opens for the chat a widget belongs to, bringing that chat to the front", async () => {
    const store = await render()
    await act(async () => void store.dispatch(openBeside({ sessionId: "b" })))
    await frames()
    await act(
      async () =>
        void store.dispatch(
          openWidget({ widget: sampleWidgets.notes, place: "panel", origin: "a" }),
        ),
    )
    await frames()
    expect(layout(store).focused).toBe(firstPane(store))
    expect(tabTitles()).toEqual(["Sample notes"])
    // Opened for a chat no pane shows, it takes the focused pane's place.
    await act(
      async () =>
        void store.dispatch(
          openWidget({ widget: sampleWidgets.trail, place: "panel", origin: "c" }),
        ),
    )
    await frames()
    expect(shown(store)).toEqual(["c", "b"])
    expect(tabTitles()).toEqual(["Sample trail"])
    expect(inFront().querySelector("nav")?.textContent).toBe("Session aSample trail")
  })

  it("draws a line with close for a widget it cannot show", async () => {
    const store = await render()
    for (const [widget, said] of [
      [sampleWidgets.off, "Turned off in Settings › Advanced › Experimental"],
      [sampleWidgets.missing, "This is no longer available"],
    ] as const) {
      await act(
        async () =>
          void store.dispatch(openWidget({ widget, place: "panel", origin: "a" })),
      )
      await frames()
      expect(inFront().textContent).toContain(said)
      expect(inFront().querySelector("nav")?.textContent).toBe("Sample")
      await click(button(inFront().querySelector(".widget-body"), "Close"))
      expect(panelOf(store, "a")).toBeUndefined()
    }
  })

  it("keeps Escape to the view's steps back while focus is inside it, and never closes the tab", async () => {
    const store = await render()
    const shownPanel = await openTrail(store)
    await click(shownPanel.querySelector('[data-sample-step="1"]'))
    expect(shownPanel.querySelector("[data-sample-detail]")).not.toBeNull()
    expect(caret()?.hasAttribute("data-sample-back")).toBe(true)
    await escape()
    expect(shownPanel.querySelector("[data-sample-detail]")).toBeNull()
    // The detail's Back held the caret; gone, the caret falls back to the widget's body.
    expect(caret()).toBe(bodyOf(shownPanel))
    await escape()
    expect(panelOf(store, "a")?.tabs).toEqual([sampleWidgets.trail])
    // With focus outside the panel, its steps back are not Escape's.
    await click(shownPanel.querySelector('[data-sample-step="0"]'))
    await act(async () => store.dispatch(focusPane({ pane: firstPane(store) })))
    host.querySelector<HTMLElement>(".desktop-composer textarea")?.focus()
    await frames()
    await escape()
    expect(shownPanel.querySelector("[data-sample-detail]")).not.toBeNull()
  })
})

describe("the session's header", () => {
  it("draws each plugin's accessory for its session, which opens in that session's panel", async () => {
    const store = await render()
    const accessory = host.querySelector("[data-sample-accessory]")
    expect(accessory?.textContent).toBe("Notes")
    await click(accessory)
    expect(shown(store)).toEqual(["a"])
    expect(panelOf(store, "a")?.tabs).toEqual([sampleWidgets.notes])
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
    const shownPanel = await openTrail(store)
    await click(button(shownPanel, "Open in Window"))
    expect(content(store)).toEqual({ widget: sampleWidgets.trail })
    expect(host.querySelector(".workspace")?.getAttribute("data-content")).toBe("widget")
    expect(caret()).toBe(bodyOf(windowShown()))
  }

  it("is left by Escape and by its close, back to the panes as they were, the caret in the focused pane", async () => {
    const store = await render()
    await openWindow(store)
    await escape()
    expect(content(store)).toBe("panes")
    expect(windowShown()).toBeNull()
    expect(shown(store)).toEqual(["a"])
    expect(caret()?.tagName).toBe("TEXTAREA")
    await click(button(inFront(), "Open in Window"))
    await click(button(theWindow(), "Close"))
    expect(content(store)).toBe("panes")
  })

  it("is closed by ⌘W, never a pane beneath it; left for a session chosen; and ⌘0 goes to the overview", async () => {
    const store = await render()
    await openWindow(store)
    await act(async () => press("closePane"))
    await frames()
    expect(content(store)).toBe("panes")
    expect(shown(store)).toEqual(["a"])
    await click(button(inFront(), "Open in Window"))
    await act(async () => void store.dispatch(openBeside({ sessionId: "b" })))
    await frames()
    expect(content(store)).toBe("panes")
    await act(async () => void store.dispatch(focusPane({ pane: firstPane(store) })))
    await frames()
    await click(button(inFront(), "Open in Window"))
    await act(async () => press("showOverview"))
    await frames()
    expect(content(store)).toBe("agents")
    // ⌘1 on the pane already focused goes back to the panes all the same.
    await act(async () => press("focusPane1"))
    await frames()
    expect(content(store)).toBe("panes")
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
    await click(button(inFront(), "Open in Window"))
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
