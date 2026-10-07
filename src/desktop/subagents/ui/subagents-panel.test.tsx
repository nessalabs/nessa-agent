// @vitest-environment jsdom
/**
 * The panel: the sample list in the model's order, a read-only conversation,
 * Escape and a child that leaves, and opening one from outside the panel.
 */
import { act, useState } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import type { WidgetHost } from "../../widgets/ui/plugin"
import type { OpenPlace } from "../../widgets/model/widget-state"
import { ClockProvider } from "../../workspace/adapters/dom/clock"
import { loadWorkspace } from "../../workspace"
import { testStore } from "../../workspace/testing"
import { SubagentsProvider } from "../adapters/react/source-context"
import { sampleSubagentSource } from "../adapters/in-memory/sample-source"
import {
  joinedSubagentId,
  subagentsPluginId,
  type SubagentRead,
  type SubagentSource,
} from "../application/ports"
import { clearSubagentChoices, selectedSubagent } from "../application/selection"
import type { Subagent } from "../model/subagent"
import { retryBudgetSession } from "../../workspace/adapters/in-memory/sample-labs"
import { subagentsPlugin } from "./plugin"
import { SubagentsPanel } from "./subagents-panel"
import { useOpenSubagent } from "./use-open-subagent"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  Element.prototype.animate ??= (() => ({
    finished: Promise.resolve(),
    cancel() {},
    finish() {},
    addEventListener() {},
    removeEventListener() {},
  })) as unknown as typeof Element.prototype.animate
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent() {
      return false
    },
    onchange: null,
  })) as unknown as typeof window.matchMedia
  localStorage.removeItem("nessa.desktop.subagents")
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
  clearSubagentChoices()
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
  clearSubagentChoices()
})

function child(id: string, activity: Subagent["activity"] = "working"): Subagent {
  return {
    id,
    name: id,
    seed: id,
    tags: [],
    headline: `${id} headline`,
    activity,
    lifecycle: "open",
    since: 0,
    conversation: {
      messages: [
        {
          id: `${id}-1`,
          role: "agent",
          at: 0,
          parts: [{ kind: "text", text: `${id} said` }],
        },
      ],
      activity: null,
    },
  }
}

function memory(
  initial: readonly Subagent[],
  unreadable: readonly string[] = [],
): SubagentSource & {
  set(next: readonly Subagent[]): void
} {
  let snapshot: SubagentRead = { kind: "ready", subagents: initial, unreadable }
  const listeners = new Set<() => void>()
  return {
    set(next) {
      snapshot = { kind: "ready", subagents: next, unreadable }
      for (const listener of listeners) listener()
    },
    forSession() {
      return snapshot
    },
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
  }
}

function widgetHost(onEscape?: (step: () => void) => void): WidgetHost & {
  readonly opened: unknown[]
} {
  const opened: unknown[] = []
  return {
    opened,
    open() {},
    close() {},
    openWidget(widget, place) {
      opened.push([widget, place])
    },
    onEscape(step) {
      onEscape?.(step)
      return () => {}
    },
  }
}

async function renderPanel(
  sessionId: string,
  source: SubagentSource,
  widget: WidgetHost,
) {
  await act(async () =>
    root.render(
      <ClockProvider now={() => 1_000_000}>
        <SubagentsProvider source={source}>
          <SubagentsPanel sessionId={sessionId} host={widget} />
        </SubagentsProvider>
      </ClockProvider>,
    ),
  )
}

describe("the subagents panel", () => {
  it("lists the sample in activity order, and opens a conversation with no composer", async () => {
    const source = sampleSubagentSource({ now: () => 1_000_000, after: () => () => {} })
    await renderPanel(retryBudgetSession, source, widgetHost())
    const rows = [...host.querySelectorAll("[data-subagent-row]")].map((row) =>
      row.getAttribute("data-subagent-name"),
    )
    expect(rows).toEqual(["Mara", "Idris", "Nia", "Sol"])
    expect(host.querySelector("[data-subagent-summary]")?.textContent).toBe(
      "1 working · 1 planning · 1 stuck · 1 closed",
    )
    expect(host.textContent).toContain("claude-sonnet-5")
    expect(host.textContent).toContain("Closed")
    expect(host.querySelector("textarea")).toBeNull()
    const mara = host.querySelector<HTMLButtonElement>("[data-subagent-row]")
    await act(async () => mara?.click())
    expect(host.querySelector("[data-subagent-detail]")?.textContent).toContain(
      "Three of the five cases hold.",
    )
    expect(host.querySelector("[data-subagent-detail] textarea")).toBeNull()
    expect(document.activeElement?.closest("[data-subagent-detail]")).not.toBeNull()
    source.dispose()
  })

  it("says a source could not be read, and names a failed key under the list", async () => {
    const failedRead: SubagentRead = { kind: "failed", failure: { kind: "unavailable" } }
    const failed: SubagentSource = {
      forSession: () => failedRead,
      subscribe: () => () => {},
    }
    await renderPanel("a", failed, widgetHost())
    expect(host.textContent).toContain("Subagents could not be read.")
    expect(host.querySelector("[data-subagent-empty]")).toBeNull()

    await renderPanel("a", memory([], ["other"]), widgetHost())
    expect(host.textContent).toContain("Could not read other.")
    expect(host.querySelector("[data-subagent-empty]")).toBeNull()

    await renderPanel("a", memory([]), widgetHost())
    expect(host.querySelector("[data-subagent-empty]")?.textContent).toContain(
      "No subagents",
    )
  })

  it("steps back on Escape, and leaves the list when the open child is gone", async () => {
    let back: (() => void) | null = null
    const source = memory([child("mara"), child("idris", "planning")])
    await renderPanel(
      "a",
      source,
      widgetHost((step) => (back = step)),
    )
    await act(async () =>
      host.querySelector<HTMLButtonElement>("[data-subagent-row]")?.click(),
    )
    expect(host.querySelector("[data-subagent-detail]")).not.toBeNull()
    await act(async () => back?.())
    expect(host.querySelector("[data-subagent-list]")).not.toBeNull()
    expect(selectedSubagent("a")).toBeNull()

    await act(async () =>
      host.querySelector<HTMLButtonElement>("[data-subagent-row]")?.click(),
    )
    await act(async () => {
      source.set([child("idris", "planning")])
    })
    expect(host.querySelector("[data-subagent-detail]")).toBeNull()
    expect(host.querySelector("[data-subagent-list]")?.textContent).toContain("idris")
    expect(selectedSubagent("a")).toBeNull()
  })
})

describe("useOpenSubagent", () => {
  it("chooses the joined id and asks the host to open the panel", async () => {
    const widget = widgetHost()
    function Opener() {
      const [place] = useState<OpenPlace>("pane")
      const open = useOpenSubagent(widget, place)
      return (
        <button
          type="button"
          onClick={() => open({ sessionId: "a", sourceKey: "sample", sourceId: "mara" })}
        >
          Open Mara
        </button>
      )
    }
    await act(async () => root.render(<Opener />))
    await act(async () => host.querySelector("button")?.click())
    expect(selectedSubagent("a")).toBe(joinedSubagentId("sample", "mara"))
    expect(widget.opened).toEqual([[{ plugin: subagentsPluginId, id: "a" }, "pane"]])
  })
})

describe("the subagents plugin's answer", () => {
  it("is ready for a listed conversation, missing once the index lacks it, and off when the preview is", async () => {
    const store = testStore()
    await store.dispatch(loadWorkspace())
    const ready: SubagentRead = { kind: "ready", subagents: [], unreadable: [] }
    const source: SubagentSource = {
      forSession: () => ready,
      subscribe: () => () => {},
    }
    const plugin = subagentsPlugin()
    function Probe({ id }: { id: string }) {
      const state = plugin.useWidget(id)
      return <output>{state.kind}</output>
    }
    await act(async () =>
      root.render(
        <Provider store={store}>
          <SubagentsProvider source={source}>
            <Probe id="a" />
          </SubagentsProvider>
        </Provider>,
      ),
    )
    expect(host.textContent).toBe("ready")

    await act(async () =>
      root.render(
        <Provider store={store}>
          <SubagentsProvider source={source}>
            <Probe id="missing" />
          </SubagentsProvider>
        </Provider>,
      ),
    )
    expect(host.textContent).toBe("missing")

    localStorage.setItem("nessa.desktop.subagents", "off")
    await act(async () =>
      root.render(
        <Provider store={store}>
          <SubagentsProvider source={source}>
            <Probe key="off" id="a" />
          </SubagentsProvider>
        </Provider>,
      ),
    )
    expect(host.textContent).toBe("off")
    localStorage.removeItem("nessa.desktop.subagents")
  })
})
