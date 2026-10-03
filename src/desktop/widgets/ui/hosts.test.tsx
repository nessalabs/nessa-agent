// @vitest-environment jsdom
/**
 * The hosts draw ADR 326's table: a card in a message, and a widget's body in
 * a pane or the window — the plugin's view, its row, its placeholder, or one
 * line — following the registry as plugins come and go at run time.
 */
import { act, StrictMode, useState } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { fixtureAppPlugin } from "../app/fixture/fixture-plugin"
import { appPlugin } from "../app/ui/app-plugin"
import { createWidgetRegistry } from "../application/registry"
import { WidgetRegistryProvider } from "../adapters/react/registry-context"
import type { WidgetRef } from "../model/widget-ref"
import type { OpenPlace, WidgetState } from "../model/widget-state"
import { InlineWidget } from "./inline-widget"
import type {
  DesktopWidgetRegistry,
  NativeWidgetPlugin,
  WidgetHost,
  WidgetPlugin,
  WidgetViewProps,
} from "./plugin"
import { WidgetAnswerOf } from "./widget-answer"
import { WidgetBody } from "./widget-body"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

const states: Record<string, WidgetState> = {
  run: { kind: "ready", title: "Checkout climb", origin: "a" },
  waiting: { kind: "unread" },
  gone: { kind: "missing" },
  off: { kind: "off" },
  broken: { kind: "unshowable" },
}

const view = (place: string) =>
  function View({ id, place: drawnIn }: WidgetViewProps) {
    return <p data-view={place}>{`${place} view of ${id} in ${drawnIn}`}</p>
  }

/** A native plugin answering from `states`; `views` says which places it draws. */
function plugin(
  views: Partial<Record<"inline" | "window", boolean>> = { inline: true, window: true },
  id = "test",
): NativeWidgetPlugin {
  return {
    kind: "native",
    id,
    name: "Experiment",
    useWidget: (widget) => (Object.hasOwn(states, widget) ? states[widget] : states.gone),
    views: {
      pane: view("pane"),
      inline: views.inline ? view("inline") : undefined,
      window: views.window ? view("window") : undefined,
    },
  }
}

function fakeHost(): WidgetHost & { opened: OpenPlace[]; closed: number } {
  const opened: OpenPlace[] = []
  const fake = {
    opened,
    closed: 0,
    open: (place: OpenPlace) => void opened.push(place),
    close: () => void fake.closed++,
    openWidget: () => {},
    onEscape: () => () => {},
  }
  return fake
}

const ref = (id: string, pluginId = "test"): WidgetRef => ({ plugin: pluginId, id })

async function draw(registry: DesktopWidgetRegistry | null, children: React.ReactNode) {
  await act(async () =>
    root.render(
      <StrictMode>
        {registry ? (
          <WidgetRegistryProvider registry={registry}>{children}</WidgetRegistryProvider>
        ) : (
          children
        )}
      </StrictMode>,
    ),
  )
}

function Body({
  widget,
  place,
  fake,
}: {
  widget: WidgetRef
  place: OpenPlace
  fake: WidgetHost
}) {
  return (
    <WidgetAnswerOf widget={widget}>
      {(answer, found) => (
        <WidgetBody
          id={widget.id}
          place={place}
          answer={answer}
          plugin={found}
          host={fake}
        />
      )}
    </WidgetAnswerOf>
  )
}

const registryOf = (...natives: NativeWidgetPlugin[]) =>
  createWidgetRegistry<WidgetPlugin>(natives)
const text = () => host.textContent?.replace(/\s+/g, " ").trim()

describe("a card in a message", () => {
  it("draws the plugin's inline view for a ready widget", async () => {
    await draw(
      registryOf(plugin()),
      <InlineWidget widget={ref("run")} host={fakeHost()} />,
    )
    expect(text()).toBe("inline view of run in inline")
  })

  it("draws a row with the title and Open for a plugin with no inline view, which opens a pane", async () => {
    const fake = fakeHost()
    await draw(
      registryOf(plugin({ window: true })),
      <InlineWidget widget={ref("run")} host={fake} />,
    )
    expect(host.querySelector(".widget-inline-title")?.textContent).toBe("Checkout climb")
    await act(async () => host.querySelector("button")?.click())
    expect(fake.opened).toEqual(["pane"])
  })

  it("draws each row of the table that is not ready, with no close", async () => {
    for (const [id, said] of [
      ["waiting", "Experiment"],
      ["gone", "This is no longer available"],
      ["off", "Turned off in Settings › Advanced › Experimental"],
      ["broken", "Can't show this here"],
    ]) {
      await draw(
        registryOf(plugin()),
        <InlineWidget widget={ref(id)} host={fakeHost()} />,
      )
      expect(text(), id).toBe(said)
      expect(host.querySelector("button"), id).toBeNull()
    }
  })

  it("says it cannot show a widget of a plugin the window does not have, or with no registry", async () => {
    await draw(
      registryOf(plugin()),
      <InlineWidget widget={ref("run", "absent")} host={fakeHost()} />,
    )
    expect(text()).toBe("Can't show this here")
    await draw(null, <InlineWidget widget={ref("run")} host={fakeHost()} />)
    expect(text()).toBe("Can't show this here")
  })
})

describe("a widget's body, in a pane or the window", () => {
  it("draws the plugin's view for the place, given the place", async () => {
    for (const place of ["pane", "window"] as const) {
      await draw(
        registryOf(plugin()),
        <Body widget={ref("run")} place={place} fake={fakeHost()} />,
      )
      expect(text()).toBe(`${place} view of run in ${place}`)
      expect(host.querySelector("[data-widget-body]")?.getAttribute("tabindex")).toBe(
        "-1",
      )
    }
  })

  it("says it cannot show a window its plugin does not draw, with close", async () => {
    const fake = fakeHost()
    await draw(
      registryOf(plugin({})),
      <Body widget={ref("run")} place="window" fake={fake} />,
    )
    expect(host.querySelector('[data-slot="empty-state-title"]')?.textContent).toBe(
      "Can't show this here",
    )
    await act(async () => host.querySelector("button")?.click())
    expect(fake.closed).toBe(1)
  })

  it("draws a placeholder with the plugin's name while not read, with no close", async () => {
    await draw(
      registryOf(plugin()),
      <Body widget={ref("waiting")} place="pane" fake={fakeHost()} />,
    )
    expect(host.querySelector('[role="status"]')?.textContent).toBe("Experiment")
    expect(host.querySelector("button")).toBeNull()
  })

  it("draws each line with close, which closes it where it is", async () => {
    for (const [widget, said] of [
      [ref("gone"), "This is no longer available"],
      [ref("off"), "Turned off in Settings › Advanced › Experimental"],
      [ref("broken"), "Can't show this here"],
      [ref("run", "absent"), "Can't show this here"],
    ] as const) {
      const fake = fakeHost()
      await draw(registryOf(plugin()), <Body widget={widget} place="pane" fake={fake} />)
      expect(host.querySelector('[data-slot="empty-state-title"]')?.textContent).toBe(
        said,
      )
      await act(async () => host.querySelector("button")?.click())
      expect(fake.closed, said).toBe(1)
    }
  })
})

describe("plugins registered while the window runs", () => {
  it("draws a widget again as its plugin is registered and unregistered", async () => {
    const registry = registryOf()
    await draw(
      registry,
      <Body widget={ref("run", "mcp:rows")} place="pane" fake={fakeHost()} />,
    )
    expect(text()).toBe("Can't show this hereClose")
    // An MCP App, whose calls hold no widget "run": it says so, by the table.
    await act(
      async () =>
        void registry.register(
          appPlugin({
            server: "rows",
            name: "Rows",
            ports: fixtureAppPlugin({
              sessionId: "a",
              sandbox: undefined,
              timers: { after: () => () => {} },
              newId: () => crypto.randomUUID(),
              page: () => ({ styles: {}, timeZone: "UTC", platform: "web" }),
            }).ports,
          }),
        ),
    )
    expect(host.querySelector('[data-slot="empty-state-title"]')?.textContent).toBe(
      "This is no longer available",
    )
    let named = ""
    await draw(
      registry,
      <WidgetAnswerOf widget={ref("run", "mcp:rows")}>
        {(answer) => {
          named = answer.registered ? answer.name : "none"
          return null
        }}
      </WidgetAnswerOf>,
    )
    expect(named).toBe("Rows")
    await act(async () => void registry.unregister("mcp:rows"))
    expect(named).toBe("none")
  })

  it("reads a widget under a fresh reader when the plugin under its id is another", async () => {
    // Each reader calls the one hook it began with: a different plugin under
    // the same id mounts a new reader rather than changing the hook it calls.
    // Two plugins whose hooks call different hooks: one reader calling both
    // would break React's order of hooks.
    const first = vi.fn((): WidgetState => {
      const [title] = useState("First")
      return { kind: "ready", title }
    })
    const second = vi.fn((): WidgetState => {
      const [title] = useState("Second")
      const [suffix] = useState("")
      return { kind: "ready", title: title + suffix }
    })
    const native = (useWidget: () => WidgetState): NativeWidgetPlugin => ({
      ...plugin(),
      useWidget,
    })
    let swap = (_: NativeWidgetPlugin) => {}
    function Swapping() {
      const [registry, setRegistry] = useState(() => registryOf(native(first)))
      swap = (next) => setRegistry(registryOf(next))
      return (
        <WidgetRegistryProvider registry={registry}>
          <WidgetAnswerOf widget={ref("run")}>
            {(answer) =>
              answer.registered && answer.state.kind === "ready" ? answer.state.title : ""
            }
          </WidgetAnswerOf>
        </WidgetRegistryProvider>
      )
    }
    await act(async () => root.render(<Swapping />))
    expect(text()).toBe("First")
    await act(async () => swap(native(second)))
    expect(text()).toBe("Second")
    expect(second).toHaveBeenCalled()
  })
})
