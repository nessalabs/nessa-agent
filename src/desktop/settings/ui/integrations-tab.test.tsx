// @vitest-environment jsdom
/**
 * Settings › Integrations as drawn: the rows of the design's table the
 * reducer cannot see (#391 PR 3) — the pending row with no gateway (U1), no
 * request without the grant (U2), skeletons (U3), the rows (U5, U6, U27),
 * stored values (U10), and no variable value left in the DOM (U31) — and one
 * request for each the reducer names, under StrictMode too.
 */
import { act, StrictMode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import type {
  McpServersConnection,
  McpServersGateway,
} from "../adapters/mcp-servers-gateway"
import type { Outcome, SaveRequest, ServerList } from "../model/mcp-servers"
import { IntegrationsTab, McpServersProvider } from "./integrations-tab"

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

const charts = {
  name: "charts",
  command: "/usr/bin/node",
  args: ["server.mjs"],
  envNames: ["TOKEN"],
  enabled: true,
  managed: false,
}
const nessa = {
  name: "nessa",
  command: "/Applications/Nessa",
  args: ["mcp"],
  envNames: [],
  enabled: true,
  managed: true,
}

/** A gateway the test answers by hand: each request waits until it says. */
function fakeGateway(mayManage = true) {
  const requests: {
    method: string
    argument?: unknown
    answer: (o: Outcome<unknown>) => void
  }[] = []
  let tell: (connection: McpServersConnection) => void = () => {}
  const ask = (method: string, argument?: unknown) =>
    new Promise<Outcome<never>>((resolve) =>
      requests.push({
        method,
        argument,
        answer: resolve as (o: Outcome<unknown>) => void,
      }),
    )
  const gateway: McpServersGateway = {
    limits: { inspectDeadlineMs: 30_000 },
    follow(handler) {
      tell = handler
      handler({ type: "connected", mayManage })
      return () => {}
    },
    list: () => ask("list"),
    save: (request: SaveRequest) => ask("save", request),
    remove: (request) => ask("remove", request),
    inspect: (name) => ask("inspect", name),
  }
  return {
    gateway,
    requests,
    tell: (connection: McpServersConnection) => tell(connection),
  }
}

async function mount(gateway: McpServersGateway | undefined) {
  await act(async () =>
    root.render(
      <StrictMode>
        <McpServersProvider gateway={gateway}>
          <IntegrationsTab />
        </McpServersProvider>
      </StrictMode>,
    ),
  )
}

async function answer(
  fake: ReturnType<typeof fakeGateway>,
  method: string,
  outcome: Outcome<unknown>,
) {
  const open = fake.requests.filter((each) => each.method === method).at(-1)
  if (!open) throw new Error(`no ${method} request`)
  await act(async () => open.answer(outcome))
}

const button = (text: string, within: ParentNode = host) =>
  [...within.querySelectorAll("button")].find((each) => each.textContent === text)

const row = (name: string) => {
  const found = host.querySelector(`[data-mcp-server="${name}"]`)
  if (!found) throw new Error(`no row ${name}`)
  return found
}
const form = () => host.querySelector("[data-mcp-form]") ?? host

async function press(element: Element | null, key: string) {
  if (!element) throw new Error("nothing to press on")
  await act(async () => {
    element.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }))
  })
}

const list = (...servers: (typeof charts)[]): Outcome<ServerList> => ({
  ok: true,
  value: { revision: "r1", servers },
})

async function click(element: Element | undefined) {
  if (!element) throw new Error("no such element")
  await act(async () => (element as HTMLElement).click())
}

async function type(element: Element | null, value: string) {
  const input = element as HTMLInputElement
  const setter = Object.getOwnPropertyDescriptor(
    Object.getPrototypeOf(input),
    "value",
  )?.set
  if (!setter) throw new Error("no value setter")
  await act(async () => {
    setter.call(input, value)
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
}

describe("Integrations", () => {
  it("U1: with no gateway, the pending row and nothing that looks as if it works", async () => {
    await mount(undefined)
    const group = host.querySelector('[data-setting="mcp-servers"]')
    expect(group?.hasAttribute("data-pending")).toBe(true)
    expect(button("Add server…")?.disabled).toBe(true)
  })

  it("U2: without the grant, the notice and no request at all", async () => {
    const fake = fakeGateway(false)
    await mount(fake.gateway)
    expect(host.textContent).toContain("Only an administrator can manage MCP servers.")
    expect(fake.requests).toEqual([])
    expect(host.querySelector("button")).toBeNull()
  })

  it("U3: skeleton rows while listing, Add disabled, and one list under StrictMode", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    expect(host.querySelectorAll("[data-mcp-skeleton]").length).toBe(2)
    expect(button("Add server…")?.disabled).toBe(true)
    expect(fake.requests.map((each) => each.method)).toEqual(["list"])
  })

  it("U5: only the managed server: no servers yet, Add, the managed row below", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", list(nessa))
    expect(host.querySelector("[data-mcp-empty]")?.textContent).toContain(
      "No servers yet",
    )
    expect(button("Add server…")?.disabled).toBe(false)
    expect(host.querySelector('[data-mcp-server="nessa"]')).not.toBeNull()
  })

  it("U6 and U27: a row per server, stored first; the managed one has nothing editable", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", list(charts, nessa))
    const rows = [...host.querySelectorAll("[data-mcp-server]")]
    expect(rows.map((row) => row.getAttribute("data-mcp-server"))).toEqual([
      "charts",
      "nessa",
    ])
    const [row, managed] = rows
    expect(row.querySelector("code")?.textContent).toBe("/usr/bin/node server.mjs")
    expect(row.textContent).toContain("1 variable")
    expect(row.querySelector('[role="switch"]')?.getAttribute("aria-checked")).toBe(
      "true",
    )
    expect(
      ["Edit", "Inspect", "Remove"].map((text) => Boolean(button(text, row))),
    ).toEqual([true, true, true])
    expect(managed.textContent).toContain("Managed by Nessa")
    expect(managed.querySelectorAll("button:not([role=switch])").length).toBe(0)
    expect((managed.querySelector('[role="switch"]') as HTMLButtonElement).disabled).toBe(
      true,
    )
  })

  it("U10 and U31: a stored value shows as kept; a typed one is nowhere in the DOM after the save", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", list(charts, nessa))
    await click(button("Edit", host.querySelector('[data-mcp-server="charts"]') ?? host))
    const value = host.querySelector(
      '[data-mcp-variable="TOKEN"] input[type="password"]',
    ) as HTMLInputElement
    expect(value.value).toBe("")
    expect(value.placeholder).toBe("Stored value kept")
    await type(value, "very-secret-value")
    await click(button("Save"))
    // U21: in flight, the form rests.
    expect((host.querySelector("fieldset") as HTMLFieldSetElement).disabled).toBe(true)
    const save = fake.requests.find((each) => each.method === "save")
    expect(save?.argument).toMatchObject({
      server: { env: [{ name: "TOKEN", value: "very-secret-value" }] },
    })
    await answer(fake, "save", { ok: true, value: undefined })
    await answer(fake, "list", list(charts, nessa))
    expect(host.querySelector("[data-mcp-form]")).toBeNull()
    expect(host.innerHTML).not.toContain("very-secret-value")
    const values = [...host.querySelectorAll("input, textarea")].map(
      (each) => (each as HTMLInputElement).value,
    )
    expect(values.join("\n")).not.toContain("very-secret-value")
    expect(fake.requests.map((each) => each.method)).toEqual(["list", "save", "list"])
  })

  it("U9 and U31: a refusal is said in a region drawn before it, at its field, and a typed value stays out of the markup", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", list(charts, nessa))
    await click(button("Edit", row("charts")))
    const command = host.querySelector(
      "[data-mcp-form] textarea.settings-input-wrapped",
    ) as HTMLTextAreaElement
    const problem = host.querySelector('[data-mcp-problem="command"]')
    // Drawn, empty, before any refusal: what arrives in it is read out.
    expect(problem?.getAttribute("role")).toBe("alert")
    expect(problem?.textContent).toBe("")
    expect(command.getAttribute("aria-describedby")).toBeNull()
    const value = host.querySelector(
      '[data-mcp-variable="TOKEN"] input[type="password"]',
    ) as HTMLInputElement
    await type(value, "kept-after-refusal")
    await click(button("Save"))
    await answer(fake, "save", {
      ok: false,
      failure: { kind: "invalid", problem: "command" },
    })
    // The same element, now holding the problem, and the field names it.
    expect(host.querySelector('[data-mcp-problem="command"]')).toBe(problem)
    expect(problem?.textContent).toBe("This command can't be used.")
    expect(command.getAttribute("aria-describedby")).toBe(problem?.id)
    expect(command.getAttribute("aria-invalid")).toBe("true")
    // The form is kept, the value in its field, and nowhere in the markup.
    expect(host.querySelector("[data-mcp-form]")).not.toBeNull()
    expect(value.value).toBe("kept-after-refusal")
    expect(host.innerHTML).not.toContain("kept-after-refusal")
  })

  it("the notices' live region is drawn before what it says", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    const region = host.querySelector("[data-mcp-notices]")
    expect(region?.getAttribute("role")).toBe("status")
    expect(region?.textContent).toBe("")
    await answer(fake, "list", { ok: false, failure: { kind: "configInvalid" } })
    expect(host.querySelector("[data-mcp-notices]")).toBe(region)
    expect(region?.textContent).toBe(
      "The configuration file can't be read as it is, so nothing was changed.",
    )
    await act(async () => fake.tell({ type: "unreachable" }))
    expect(host.querySelector("[data-mcp-notices]")).toBe(region)
    expect(region?.querySelector("[data-mcp-unreachable]")).not.toBeNull()
  })

  it("the inspection's status is one live region, drawn with the panel, that its answer arrives in", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", list(charts, nessa))
    await click(button("Inspect", row("charts")))
    const panel = host.querySelector("[data-mcp-inspection]")
    expect(panel?.hasAttribute("aria-live")).toBe(false)
    const status = host.querySelector("[data-mcp-inspection-status]")
    expect(status?.getAttribute("role")).toBe("status")
    expect(status?.textContent).toBe("Starting “charts”… It has up to 30 seconds.")
    await answer(fake, "inspect", {
      ok: true,
      value: { complete: true, tools: [{ name: "show_chart" }, { name: "rows" }] },
    })
    expect(host.querySelector("[data-mcp-inspection-status]")).toBe(status)
    expect(status?.textContent).toBe("It offers 2 tools.")
  })

  it("U21: Inspect rests while a write is in flight", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", list(charts, nessa))
    expect(button("Inspect", row("charts"))?.disabled).toBe(false)
    await click(row("charts").querySelector('[role="switch"]') ?? undefined)
    expect(button("Inspect", row("charts"))?.disabled).toBe(true)
  })

  describe("focus", () => {
    async function listedTab() {
      const fake = fakeGateway()
      await mount(fake.gateway)
      await answer(fake, "list", list(charts, nessa))
      return fake
    }
    const active = () => document.activeElement
    const nameField = () =>
      host.querySelector("[data-mcp-form] input") as HTMLInputElement | null

    it("Add puts it on the form's first field; Cancel, Escape and Save put it back on Add", async () => {
      const fake = await listedTab()
      await click(button("Add server…"))
      expect(active()).toBe(nameField())
      await click(button("Cancel", form()))
      expect(active()).toBe(button("Add server…"))
      await click(button("Add server…"))
      await press(nameField(), "Escape")
      expect(host.querySelector("[data-mcp-form]")).toBeNull()
      expect(active()).toBe(button("Add server…"))
      await click(button("Add server…"))
      await type(nameField(), "maps")
      await type(host.querySelector("[data-mcp-form] textarea"), "/bin/maps")
      await click(button("Save"))
      await answer(fake, "save", { ok: true, value: undefined })
      // Resting while the list is read, then focused once it is answered.
      await answer(fake, "list", list(charts, { ...charts, name: "maps" }, nessa))
      expect(active()).toBe(button("Add server…"))
    })

    it("Edit puts it on the first field; Cancel or Save puts it back on the row's Edit, under the name saved", async () => {
      const fake = await listedTab()
      await click(button("Edit", row("charts")))
      expect(active()).toBe(nameField())
      await click(button("Cancel", form()))
      expect(active()).toBe(button("Edit", row("charts")))
      await click(button("Edit", row("charts")))
      await type(nameField(), "graphs")
      await click(button("Save"))
      await answer(fake, "save", { ok: true, value: undefined })
      await answer(fake, "list", list({ ...charts, name: "graphs" }, nessa))
      expect(active()).toBe(button("Edit", row("graphs")))
    })

    it("Remove puts it on the confirm's Cancel, linked to its sentence; Cancel and Escape put it back on Remove", async () => {
      await listedTab()
      const remove = button("Remove", row("charts"))
      await click(remove)
      const cancel = button("Cancel", row("charts"))
      const ask = row("charts").querySelector("[data-mcp-confirm]")
      expect(active()).toBe(cancel)
      // Its own button, not the row's Edit reused in place.
      expect(cancel).not.toBe(button("Edit", row("charts")))
      expect(cancel?.getAttribute("data-mcp-action")).toBe("cancel")
      expect(ask?.id).toBeTruthy()
      for (const each of [
        cancel,
        row("charts").querySelector('[data-mcp-action="confirm"]'),
      ])
        expect(each?.getAttribute("aria-describedby")).toBe(ask?.id)
      await click(cancel)
      expect(active()).toBe(button("Remove", row("charts")))
      await click(button("Remove", row("charts")))
      await press(active(), "Escape")
      expect(row("charts").querySelector("[data-mcp-confirm]")).toBeNull()
      expect(active()).toBe(button("Remove", row("charts")))
    })

    it("a confirmed removal puts it on Add once the row is gone", async () => {
      const fake = await listedTab()
      await click(button("Remove", row("charts")))
      await click(row("charts").querySelector('[data-mcp-action="confirm"]') ?? undefined)
      await answer(fake, "remove", { ok: true, value: undefined })
      await answer(fake, "list", list(nessa))
      expect(active()).toBe(button("Add server…"))
    })

    it("Inspect puts it on the panel's heading; Close puts it back on the row's Inspect", async () => {
      const fake = await listedTab()
      await click(button("Inspect", row("charts")))
      expect(active()).toBe(host.querySelector("[data-mcp-inspection] h2"))
      await answer(fake, "inspect", { ok: true, value: { complete: true, tools: [] } })
      await click(button("Close"))
      expect(active()).toBe(button("Inspect", row("charts")))
    })
  })

  it("U28: unreachable says so and rests every control", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", list(charts, nessa))
    await act(async () => fake.tell({ type: "unreachable" }))
    expect(host.querySelector("[data-mcp-unreachable]")?.textContent).toBe(
      "Gateway unreachable",
    )
    const controls = [...host.querySelectorAll("button")]
    expect(controls.every((each) => each.disabled)).toBe(true)
  })
})
