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
