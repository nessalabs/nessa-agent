// @vitest-environment jsdom
/**
 * The pane header's stack: nothing when the conversation has no subagents
 * or the preview is off; the busiest first when it has some; a click opens
 * the panel beside the conversation.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { OpenPlace } from "../../widgets/model/widget-state"
import type { WidgetRef } from "../../widgets/model/widget-ref"
import { SubagentsProvider } from "../adapters/react/source-context"
import {
  subagentsPluginId,
  type SubagentRead,
  type SubagentSource,
} from "../application/ports"
import { clearSubagentChoices, selectedSubagent } from "../application/selection"
import type { Subagent, SubagentActivity } from "../model/subagent"
import { SubagentStackAccessory } from "./subagent-stack"

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
  const stored = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => stored.get(key) ?? null,
    setItem: (key: string, value: string) => {
      stored.set(key, value)
    },
    removeItem: (key: string) => {
      stored.delete(key)
    },
  } satisfies Pick<Storage, "getItem" | "setItem" | "removeItem">)
  window.localStorage.removeItem("nessa.desktop.subagents")
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
  clearSubagentChoices()
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
  clearSubagentChoices()
  vi.unstubAllGlobals()
})

function child(id: string, activity: SubagentActivity): Subagent {
  return {
    id,
    name: id,
    seed: id,
    tags: [],
    headline: "",
    activity,
    lifecycle: "open",
    since: 0,
    conversation: { messages: [], activity: null },
  }
}

function source(read: SubagentRead): SubagentSource {
  return {
    forSession: () => read,
    subscribe: () => () => {},
  }
}

const ready = (subagents: readonly Subagent[]): SubagentRead => ({
  kind: "ready",
  subagents,
  unreadable: [],
})

async function render(
  read: SubagentRead,
  openWidget: (widget: WidgetRef, place: OpenPlace) => void = () => {},
  sessionId = "a",
) {
  await act(async () =>
    root.render(
      <SubagentsProvider source={source(read)}>
        <div
          onClick={() => {
            host.dataset.headerClick = "yes"
          }}
        >
          <SubagentStackAccessory sessionId={sessionId} openWidget={openWidget} />
        </div>
      </SubagentsProvider>,
    ),
  )
}

function faces(): string[] {
  return [...host.querySelectorAll('[data-slot="random-avatar-paint"]')].map(
    (face) => face.getAttribute("aria-label") ?? "",
  )
}

describe("the pane header's subagents", () => {
  it("draws nothing when the conversation has none", async () => {
    await render(ready([]))
    expect(host.querySelector("[data-subagent-stack]")).toBeNull()
    expect(host.querySelector('[data-slot="avatar-stack"]')).toBeNull()
  })

  it("draws nothing for a conversation other than the one that has them", async () => {
    const reads: Record<string, SubagentRead> = { a: ready([child("Ada", "working")]) }
    const empty = ready([])
    await act(async () =>
      root.render(
        <SubagentsProvider
          source={{
            forSession: (id) => {
              const found = Object.hasOwn(reads, id) ? reads[id] : undefined
              return found ?? empty
            },
            subscribe: () => () => {},
          }}
        >
          <SubagentStackAccessory sessionId="b" openWidget={() => {}} />
        </SubagentsProvider>,
      ),
    )
    expect(host.querySelector("[data-subagent-stack]")).toBeNull()
  })

  it("draws the busiest first, and collapses the rest", async () => {
    await render(
      ready([
        child("Dee", "idle"),
        child("Cy", "stuck"),
        child("Bea", "planning"),
        child("Ada", "working"),
      ]),
    )
    expect(faces()).toEqual(["Ada, working", "Bea", "Cy"])
    expect(
      [...host.querySelectorAll("[data-busy]")].map((node) =>
        node
          .querySelector('[data-slot="random-avatar-paint"]')
          ?.getAttribute("aria-label"),
      ),
    ).toEqual(["Ada, working"])
    expect(host.querySelector('[data-slot="avatar-stack-more"]')?.textContent).toBe("+1")
    expect(
      host.querySelector('[data-slot="avatar-stack"]')?.getAttribute("aria-label"),
    ).toBe("4 agents")
  })

  it("draws nothing when the preview is off", async () => {
    window.localStorage.setItem("nessa.desktop.subagents", "off")
    await render(ready([child("Ada", "working")]))
    expect(host.querySelector("[data-subagent-stack]")).toBeNull()
  })

  it("opens the panel beside the conversation, and the click stays on the stack", async () => {
    const opened: unknown[] = []
    await render(ready([child("Ada", "working")]), (widget, place) => {
      opened.push([widget, place])
    })
    const stack = host.querySelector<HTMLButtonElement>("[data-subagent-stack]")
    await act(async () => stack?.click())
    expect(opened).toEqual([[{ plugin: subagentsPluginId, id: "a" }, "pane"]])
    expect(selectedSubagent("a")).toBeNull()
    expect(host.dataset.headerClick).toBeUndefined()
  })
})
