// @vitest-environment jsdom
/**
 * Settings › Integrations as drawn: the rows of the design's table the
 * reducer cannot see (#391 PR 3) — the pending row with no gateway (U1), nothing
 * offered when the list is refused forbidden (U2), skeletons (U3), the rows (U5, U6, U27),
 * stored values (U10), and no variable value left in the DOM (U31) — the
 * argument rows, a value's line breaks, a changed launch (U33), a list too
 * large to show (U44–U46), a save too large (U48), the secret field
 * (S1–S6), rows sharing a name (G1–G6), and the form's focus (F11, F12) — and one
 * request for each the reducer names, under StrictMode too.
 */
import { act, StrictMode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import type {
  McpServersConnection,
  McpServersGateway,
} from "../adapters/mcp-servers-gateway"
import type { ListedServer, Outcome, SaveRequest, ServerList } from "../model/mcp-servers"
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
function fakeGateway() {
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
      handler({ type: "connected" })
      return () => {}
    },
    list: () => ask("list"),
    save: (request: SaveRequest) => ask("save", request),
    remove: (request) => ask("remove", request),
    inspect: (name) => ask("inspect", name),
    authorize: (id, revision) => ask("authorize", { id, revision }),
    revoke: (id, revision) => ask("revoke", { id, revision }),
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

const list = (...servers: ListedServer[]): Outcome<ServerList> => ({
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

  it("U2: a list refused forbidden is the notice, no controls, and nothing more asked", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    // The gateway decides who may manage: the window asks, and goes by the answer.
    expect(fake.requests.map((each) => each.method)).toEqual(["list"])
    await answer(fake, "list", { ok: false, failure: { kind: "forbidden" } })
    expect(
      host.querySelector("[data-mcp-servers]")?.getAttribute("data-mcp-servers"),
    ).toBe("not-admin")
    expect(host.textContent).toContain("Only an administrator can manage MCP servers.")
    expect(host.querySelector("button")).toBeNull()
    expect(host.querySelector("[data-mcp-server]")).toBeNull()
    expect(fake.requests.map((each) => each.method)).toEqual(["list"])
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
      '[data-mcp-variable="TOKEN"] [data-mcp-secret]',
    ) as HTMLTextAreaElement
    expect(value.value).toBe("")
    expect(value.placeholder).toBe("Stored value kept")
    await type(value, "very-secret-value")
    await click(button("Save"))
    // U21: in flight, the form rests.
    expect(
      (host.querySelector("fieldset.settings-form") as HTMLFieldSetElement).disabled,
    ).toBe(true)
    const save = fake.requests.find((each) => each.method === "save")
    expect(save?.argument).toMatchObject({
      server: { env: [{ name: "TOKEN", value: "very-secret-value" }] },
    })
    await answer(fake, "save", { ok: true, value: { live: true } })
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
      "[data-mcp-form] [data-mcp-field=command]",
    ) as HTMLTextAreaElement
    const problem = host.querySelector('[data-mcp-problem="command"]')
    // Drawn, empty, before any refusal: what arrives in it is read out.
    expect(problem?.getAttribute("role")).toBe("alert")
    expect(problem?.textContent).toBe("")
    expect(command.getAttribute("aria-describedby")).toBeNull()
    const value = host.querySelector(
      '[data-mcp-variable="TOKEN"] [data-mcp-secret]',
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

  it("U44–U46: a list too large shows no server, removes the name typed at the refusal's revision, and says when it is not stored", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", {
      ok: false,
      failure: { kind: "configTooLarge", revision: "r7" },
    })
    const panel = host.querySelector("[data-mcp-too-large]") ?? undefined
    if (!panel) throw new Error("no too-large panel")
    expect(
      host.querySelector("[data-mcp-servers]")?.getAttribute("data-mcp-servers"),
    ).toBe("too-large")
    expect(panel?.textContent).toContain(
      "The server list is too large to show. Removing a server fixes it: enter its name.",
    )
    expect(host.querySelector("[data-mcp-server]")).toBeNull()
    expect(button("Add server…")).toBeUndefined()
    const field = host.querySelector(
      '[data-mcp-action="removeByName"]',
    ) as HTMLInputElement
    expect(button("Remove", panel)?.disabled).toBe(true)
    await type(field, "bigg")
    await click(button("Remove", panel))
    // Asked as a row's Remove is, focus on the confirm's Cancel.
    expect(panel?.querySelector("[data-mcp-confirm]")?.textContent).toBe(
      "Remove “bigg”? This removes the first server stored under that name. New conversations stop getting it. Open ones keep it until they close.",
    )
    expect(document.activeElement).toBe(button("Cancel", panel))
    await press(document.activeElement, "Escape")
    expect(panel?.querySelector("[data-mcp-confirm]")).toBeNull()
    expect(document.activeElement).toBe(field)
    await click(button("Remove", panel))
    await click(panel?.querySelector('[data-mcp-action="confirm"]') ?? undefined)
    expect(fake.requests.at(-1)).toMatchObject({
      method: "remove",
      argument: { revision: "r7", name: "bigg" },
    })
    await answer(fake, "remove", { ok: false, failure: { kind: "notFound" } })
    await answer(fake, "list", {
      ok: false,
      failure: { kind: "configTooLarge", revision: "r7" },
    })
    expect(host.querySelector("[data-mcp-notice]")?.textContent).toBe(
      "No server is stored under “bigg”.",
    )
    expect(field.value).toBe("bigg")
    await type(field, "big")
    await click(button("Remove", panel))
    await click(panel?.querySelector('[data-mcp-action="confirm"]') ?? undefined)
    await answer(fake, "remove", { ok: true, value: { live: true } })
    await answer(fake, "list", list(charts, nessa))
    expect(host.querySelector("[data-mcp-too-large]")).toBeNull()
    expect(row("charts")).not.toBeNull()
  })

  it("U48: a save too large is said in the form, which stays open with what was typed", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", list(charts, nessa))
    await click(button("Edit", row("charts")))
    await click(button("Save"))
    await answer(fake, "save", { ok: false, failure: { kind: "configTooLarge" } })
    expect(host.querySelector('[data-mcp-problem="form"]')?.textContent).toBe(
      "This would make the server list too large; remove a server or shorten its arguments.",
    )
    expect(host.querySelector("[data-mcp-form]")).not.toBeNull()
    expect(fake.requests.filter((each) => each.method === "list")).toHaveLength(1)
  })

  describe("a variable's value (S1–S6)", () => {
    async function editing() {
      const fake = fakeGateway()
      await mount(fake.gateway)
      await answer(fake, "list", list(charts, nessa))
      await click(button("Edit", row("charts")))
      return fake
    }
    const variable = () => {
      const found = host.querySelector('[data-mcp-variable="TOKEN"]')
      if (!found) throw new Error("no TOKEN variable")
      return found
    }
    const field = () =>
      variable().querySelector("[data-mcp-secret]") as HTMLInputElement | null
    const saved = (fake: ReturnType<typeof fakeGateway>) => {
      const server = (
        fake.requests.find((each) => each.method === "save")?.argument as SaveRequest
      ).server
      if (server.kind !== "stdio") throw new Error("not a command server")
      return server.env
    }
    async function paste(element: Element | null, text: string) {
      if (!element) throw new Error("nothing to paste into")
      const event = new Event("paste", { bubbles: true, cancelable: true })
      Object.defineProperty(event, "clipboardData", {
        value: { getData: (kind: string) => (kind === "text/plain" ? text : "") },
      })
      await act(async () => {
        element.dispatchEvent(event)
      })
      return event
    }

    it("S1 and S3: a password field, uncontrolled, its value out of the markup", async () => {
      const fake = await editing()
      const value = field()
      if (!value) throw new Error("no value field")
      expect(value.tagName).toBe("INPUT")
      expect(value.type).toBe("password")
      expect(value.hasAttribute("value")).toBe(false)
      expect(value.placeholder).toBe("Stored value kept")
      expect(value.getAttribute("aria-label")).toBe("Value of TOKEN")
      await type(value, "sk-typed")
      expect(host.innerHTML).not.toContain("sk-typed")
      await click(button("Save"))
      expect(saved(fake)).toEqual([{ name: "TOKEN", value: "sk-typed" }])
    })

    it("S1: untouched, the stored value is kept", async () => {
      const fake = await editing()
      await click(button("Save"))
      expect(saved(fake)).toEqual([{ name: "TOKEN", value: null }])
    })

    it("S4 and S5: a paste with line breaks is held, never drawn, its trailing break trimmed only when asked", async () => {
      const fake = await editing()
      const pem = "-----BEGIN KEY-----\r\nAAAA\nBBBB\n-----END KEY-----\n"
      const pasted = await paste(field(), pem)
      expect(pasted.defaultPrevented).toBe(true)
      expect(field()).toBeNull()
      const held = variable().querySelector("[data-mcp-secret-held]")
      expect(held?.getAttribute("role")).toBe("group")
      expect(held?.getAttribute("aria-label")).toBe("Value of TOKEN")
      expect(held?.querySelector("[data-mcp-pasted]")?.textContent).toBe(
        "Pasted value: 4 lines, ends with a line break",
      )
      expect(host.innerHTML).not.toContain("AAAA")
      expect(document.activeElement).toBe(button("Clear", variable()))
      await click(button("Remove line break", variable()))
      expect(held?.querySelector("[data-mcp-pasted]")?.textContent).toBe(
        "Pasted value: 4 lines",
      )
      expect(button("Remove line break", variable())).toBeUndefined()
      await click(button("Save"))
      expect(saved(fake)).toEqual([
        { name: "TOKEN", value: "-----BEGIN KEY-----\r\nAAAA\nBBBB\n-----END KEY-----" },
      ])
      expect(host.innerHTML).not.toContain("AAAA")
    })

    it("S5 (M2): Remove line break keeps focus: on itself while a break is left, then on Clear", async () => {
      await editing()
      await paste(field(), "sk-1\n\n")
      await click(button("Remove line break", variable()))
      expect(document.activeElement).toBe(button("Remove line break", variable()))
      await click(button("Remove line break", variable()))
      expect(button("Remove line break", variable())).toBeUndefined()
      expect(document.activeElement).toBe(button("Clear", variable()))
    })

    it("S4 (M3): text with a line break dropped on the field is held, as a paste", async () => {
      const fake = await editing()
      const drop = (text: string) => {
        const event = new Event("drop", { bubbles: true, cancelable: true })
        Object.defineProperty(event, "dataTransfer", {
          value: { getData: (kind: string) => (kind === "text/plain" ? text : "") },
        })
        return event
      }
      const plain = drop("sk-1")
      await act(async () => {
        field()?.dispatchEvent(plain)
      })
      expect(plain.defaultPrevented).toBe(false)
      const lines = drop("a\nb")
      await act(async () => {
        field()?.dispatchEvent(lines)
      })
      expect(lines.defaultPrevented).toBe(true)
      expect(variable().querySelector("[data-mcp-pasted]")?.textContent).toBe(
        "Pasted value: 2 lines",
      )
      expect(host.innerHTML).not.toContain("a\nb")
      await click(button("Save"))
      expect(saved(fake)).toEqual([{ name: "TOKEN", value: "a\nb" }])
    })

    it("S1 (M4): the field asks the browser for a new password, never a saved one", async () => {
      await editing()
      expect(field()?.getAttribute("autocomplete")).toBe("new-password")
    })

    it("S4: a trailing break left in is saved as pasted", async () => {
      const fake = await editing()
      await paste(field(), "sk-1\n")
      expect(variable().querySelector("[data-mcp-pasted]")?.textContent).toBe(
        "Pasted value: 1 line, ends with a line break",
      )
      await click(button("Save"))
      expect(saved(fake)).toEqual([{ name: "TOKEN", value: "sk-1\n" }])
    })

    it("S3: a paste with no line break is the field's own", async () => {
      await editing()
      const pasted = await paste(field(), "sk-1")
      expect(pasted.defaultPrevented).toBe(false)
      expect(field()).not.toBeNull()
      expect(variable().querySelector("[data-mcp-secret-held]")).toBeNull()
    })

    it("S2 and S6 (Codex P1): cleared, the stored value is cleared, said so; Keep stored value undoes it", async () => {
      const fake = await editing()
      await paste(field(), "a\nb")
      await click(button("Clear", variable()))
      expect(document.activeElement).toBe(field())
      expect(field()?.placeholder).toBe("Empty: the stored value will be cleared")
      await click(button("Save"))
      expect(saved(fake)).toEqual([{ name: "TOKEN", value: "" }])
    })

    it("S2: Keep stored value goes back to keeping it, the field emptied", async () => {
      const fake = await editing()
      await type(field(), "x")
      expect(button("Keep stored value", variable())).toBeDefined()
      await click(button("Keep stored value", variable()))
      expect(field()?.value).toBe("")
      expect(field()?.placeholder).toBe("Stored value kept")
      expect(document.activeElement).toBe(field())
      expect(button("Keep stored value", variable())).toBeUndefined()
      await click(button("Save"))
      expect(saved(fake)).toEqual([{ name: "TOKEN", value: null }])
    })
  })

  it("arguments are one field each: an empty one and one with a line break are sent as typed", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", list({ ...charts, args: ["", "a\nb"] }, nessa))
    await click(button("Edit", row("charts")))
    const fields = () =>
      [
        ...form().querySelectorAll("[data-mcp-argument] textarea"),
      ] as HTMLTextAreaElement[]
    expect(fields().map((each) => each.value)).toEqual(["", "a\nb"])
    await click(button("Add argument", form()))
    await type(fields()[2], "c\nd")
    await click(form().querySelector('[aria-label="Remove argument 1"]') ?? undefined)
    expect(fields().map((each) => each.value)).toEqual(["a\nb", "c\nd"])
    // The launch changed: TOKEN's value again, before Save.
    await type(host.querySelector('[data-mcp-variable="TOKEN"] [data-mcp-secret]'), "t")
    await click(button("Save"))
    expect(fake.requests.find((each) => each.method === "save")?.argument).toMatchObject({
      server: { args: ["a\nb", "c\nd"] },
    })
  })

  it("U33: a changed command asks for every stored value again, says why, and holds Save", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", list(charts, nessa))
    await click(button("Edit", row("charts")))
    const value = host.querySelector(
      '[data-mcp-variable="TOKEN"] [data-mcp-secret]',
    ) as HTMLTextAreaElement
    const note = host.querySelector("[data-mcp-values-needed]")
    expect(note?.textContent).toBe("")
    expect(button("Save")?.disabled).toBe(false)
    const command = host.querySelector("[data-mcp-field=command]")
    await type(command, "/bin/new")
    expect(value.placeholder).toBe("Enter the value again")
    expect(note?.textContent).toBe(
      "Changing the command, arguments or variables needs every stored value entered again.",
    )
    expect(value.getAttribute("aria-describedby")).toContain(note?.id)
    expect(button("Save")?.disabled).toBe(true)
    await type(value, "again")
    expect(note?.textContent).toBe("")
    expect(button("Save")?.disabled).toBe(false)
    // Emptied, it is entered as empty (S7), and says so.
    await type(value, "")
    expect(value.placeholder).toBe("Empty: the stored value will be cleared")
    expect(button("Save")?.disabled).toBe(false)
    // Back to the listed command and kept: kept again.
    await type(command, charts.command)
    await click(button("Keep stored value"))
    expect(value.placeholder).toBe("Stored value kept")
    expect(button("Save")?.disabled).toBe(false)
  })

  it("G1: rows are keyed by occurrence id: a row above going does not redraw the rest, and no key warning", async () => {
    const warnings: unknown[] = []
    const original = console.error
    console.error = (...args: unknown[]) => warnings.push(args)
    try {
      const fake = fakeGateway()
      await mount(fake.gateway)
      const docs = { ...charts, name: "docs" }
      await answer(
        fake,
        "list",
        list(
          docs,
          charts,
          { ...charts, command: "/bin/other" },
          { ...charts, name: "c" },
          nessa,
        ),
      )
      const before = row("c")
      await act(async () => fake.tell({ type: "unreachable" }))
      await act(async () => fake.tell({ type: "connected" }))
      await answer(fake, "list", list(charts, { ...charts, name: "c" }, nessa))
      // The same element: kept, not drawn again in another's place.
      expect(row("c")).toBe(before)
      expect(JSON.stringify(warnings)).not.toMatch(/same key/)
      await click(button("Inspect", row("c")))
      await answer(fake, "inspect", {
        ok: true,
        value: { complete: true, tools: [{ name: "t" }, { name: "t" }] },
      })
      expect(host.querySelectorAll('[data-mcp-tool="t"]').length).toBe(2)
      expect(JSON.stringify(warnings)).not.toMatch(/same key/)
    } finally {
      console.error = original
    }
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
      "The servers couldn't be listed: the configuration file can't be read as it is.",
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

  it("a cut inspection says why under its tools; a stopping one says it as the status, with no tools", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", list(charts, nessa))
    await click(button("Inspect", row("charts")))
    await answer(fake, "inspect", {
      ok: true,
      value: { complete: false, cut: "bytes", tools: [{ name: "show_chart" }] },
    })
    const status = () => host.querySelector("[data-mcp-inspection-status]")?.textContent
    expect(status()).toBe("It offers 1 tool.")
    expect(host.querySelector("[data-mcp-cut]")?.getAttribute("data-mcp-cut")).toBe(
      "bytes",
    )
    expect(host.querySelector("[data-mcp-cut]")?.textContent).toBe(
      "The answer was too long; tools were left off the end.",
    )
    await click(host.querySelector('[data-mcp-action="close"]') ?? undefined)
    await click(button("Inspect", row("charts")))
    await answer(fake, "inspect", {
      ok: true,
      value: { complete: false, cut: "stopping", tools: [] },
    })
    expect(status()).toBe(
      "The gateway began to stop, so the server was stopped before its tools were read.",
    )
    expect(host.querySelector("[data-mcp-cut]")).toBeNull()
    expect(host.querySelector(".settings-tools")).toBeNull()
  })

  it("U21: Inspect rests while a write is in flight", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", list(charts, nessa))
    expect(button("Inspect", row("charts"))?.disabled).toBe(false)
    await click(row("charts").querySelector('[role="switch"]') ?? undefined)
    expect(button("Inspect", row("charts"))?.disabled).toBe(true)
  })

  describe("a stored value kept only for the same launch (V2–V9)", () => {
    const two = { ...charts, envNames: ["A", "B"] }
    async function editing() {
      const fake = fakeGateway()
      await mount(fake.gateway)
      await answer(fake, "list", list(two, nessa))
      await click(button("Edit", row("charts")))
      return fake
    }
    const variable = (name: string) => {
      const found = host.querySelector(`[data-mcp-variable="${name}"]`)
      if (!found) throw new Error(`no ${name} variable`)
      return found
    }
    const value = (name: string) =>
      variable(name).querySelector("[data-mcp-secret]") as HTMLInputElement
    const note = () => host.querySelector("[data-mcp-values-needed]")?.textContent
    const again =
      "Changing the command, arguments or variables needs every stored value entered again."

    it("V3 and V7: a variable added while another is kept asks for every value; removed, kept again", async () => {
      const fake = await editing()
      await click(button("Add variable", form()))
      const added = form().querySelectorAll("[data-mcp-variable-key]")[2]
      await type(added.querySelector("input"), "C")
      await type(added.querySelector("[data-mcp-secret]"), "c")
      expect(value("A").placeholder).toBe("Enter the value again")
      expect(value("B").placeholder).toBe("Enter the value again")
      expect(note()).toBe(again)
      expect(button("Save")?.disabled).toBe(true)
      // A value entered for A is no keeping: Keep is not offered (V9).
      await type(value("A"), "a")
      expect(button("Keep stored value", variable("A"))).toBeUndefined()
      await type(value("A"), "")
      await click(form().querySelector('[aria-label="Remove C"]') ?? undefined)
      // A is still edited (empty): B waits, Keep comes back for A.
      expect(note()).toBe(again)
      expect(button("Keep stored value", variable("A"))).toBeDefined()
      await click(button("Keep stored value", variable("A")))
      expect(note()).toBe("")
      expect(value("B").placeholder).toBe("Stored value kept")
      await click(button("Save"))
      expect(fake.requests.at(-1)?.argument).toMatchObject({
        server: {
          env: [
            { name: "A", value: null },
            { name: "B", value: null },
          ],
        },
      })
    })

    it("V4: one value edited while another is kept asks for the other again", async () => {
      const fake = await editing()
      await type(value("A"), "a")
      expect(value("B").placeholder).toBe("Enter the value again")
      expect(note()).toBe(again)
      expect(button("Save")?.disabled).toBe(true)
      expect(button("Keep stored value", variable("A"))).toBeDefined()
      await type(value("B"), "b")
      expect(note()).toBe("")
      await click(button("Save"))
      expect(fake.requests.at(-1)?.argument).toMatchObject({
        server: {
          env: [
            { name: "A", value: "a" },
            { name: "B", value: "b" },
          ],
        },
      })
    })

    it("V5: a stored variable removed asks for the others again", async () => {
      await editing()
      await click(form().querySelector('[aria-label="Remove B"]') ?? undefined)
      expect(value("A").placeholder).toBe("Enter the value again")
      expect(note()).toBe(again)
      expect(button("Save")?.disabled).toBe(true)
      await type(value("A"), "a")
      expect(button("Keep stored value", variable("A"))).toBeUndefined()
      expect(button("Save")?.disabled).toBe(false)
    })
  })

  it("L5: a remove whose list didn't go live as a whole says so", async () => {
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", list(charts, nessa))
    await click(button("Remove", row("charts")))
    await click(row("charts").querySelector('[data-mcp-action="confirm"]') ?? undefined)
    await answer(fake, "remove", { ok: true, value: { live: false } })
    await answer(fake, "list", list(nessa))
    expect(host.querySelector("[data-mcp-notice]")?.textContent).toBe(
      "Removed. The rest of the list takes effect once the configuration is fixed, or the gateway starts again.",
    )
  })

  describe("rows sharing a name (G3–G6)", () => {
    const first = { ...charts, args: ["first.mjs"] }
    const second = { ...charts, args: ["second.mjs"], enabled: false }
    const third = { ...charts, args: ["third.mjs"] }
    const docs = { ...charts, name: "docs" }
    const group = () => {
      const found = host.querySelector('[data-mcp-group="charts"]')
      if (!found) throw new Error("no group")
      return found
    }
    const action = "Remove the first server named “charts”"
    async function thrice() {
      const fake = fakeGateway()
      await mount(fake.gateway)
      await answer(fake, "list", list(docs, first, second, third, nessa))
      return fake
    }

    it("G3: three servers under one name are one read-only group, with one action", async () => {
      await thrice()
      expect(host.querySelectorAll('[data-mcp-server="charts"]').length).toBe(0)
      expect(host.querySelectorAll("[data-mcp-group]").length).toBe(1)
      const shared = group().querySelector("[data-mcp-shared]")
      expect(shared?.textContent).toBe(
        "3 servers share this name. Only the first can be removed here, and none edited.",
      )
      const rows = [...group().querySelectorAll("[data-mcp-shared-row]")]
      expect(rows.map((each) => each.textContent)).toEqual([
        "/usr/bin/node first.mjs1 variable",
        "/usr/bin/node second.mjs1 variable · Not offered",
        "/usr/bin/node third.mjs1 variable",
      ])
      // No Edit, Inspect, Remove of a row, or switch: one action.
      expect(group().querySelectorAll("button").length).toBe(1)
      expect(group().querySelector('[role="switch"]')).toBeNull()
      expect(button(action, group())?.disabled).toBe(false)
      expect(button(action, group())?.getAttribute("aria-describedby")).toBe(shared?.id)
      // Groups keep the first one's place: docs, then charts, then Add.
      expect(
        [...host.querySelectorAll("[data-mcp-server], [data-mcp-group]")].map(
          (each) =>
            each.getAttribute("data-mcp-server") ?? each.getAttribute("data-mcp-group"),
        ),
      ).toEqual(["docs", "charts", "nessa"])
      expect(button("Edit", row("docs"))?.disabled).toBe(false)
    })

    it("G5: the action asks, saying exactly what goes; Cancel and Escape put focus back on it", async () => {
      await thrice()
      await click(button(action, group()))
      expect(group().querySelector("[data-mcp-confirm]")?.textContent).toBe(
        "Remove the first server named “charts”, which runs /usr/bin/node? New conversations stop getting it. Open ones keep it until they close.",
      )
      expect(document.activeElement).toBe(button("Cancel", group()))
      await click(button("Cancel", group()))
      expect(document.activeElement).toBe(button(action, group()))
      await click(button(action, group()))
      await press(document.activeElement, "Escape")
      expect(group().querySelector("[data-mcp-confirm]")).toBeNull()
      expect(document.activeElement).toBe(button(action, group()))
    })

    it("G6: confirmed, the name is sent; focus goes to the group while it lasts, then the row left", async () => {
      const fake = await thrice()
      await click(button(action, group()))
      await click(group().querySelector('[data-mcp-action="confirm"]') ?? undefined)
      expect(fake.requests.at(-1)).toMatchObject({
        method: "remove",
        argument: { revision: "r1", name: "charts" },
      })
      await answer(fake, "remove", { ok: true, value: { live: true } })
      await answer(fake, "list", list(docs, second, third, nessa))
      expect(group().querySelector("[data-mcp-shared]")?.textContent).toContain(
        "2 servers",
      )
      expect(document.activeElement).toBe(button(action, group()))
      await click(button(action, group()))
      await click(group().querySelector('[data-mcp-action="confirm"]') ?? undefined)
      await answer(fake, "remove", { ok: true, value: { live: true } })
      await answer(fake, "list", list(docs, third, nessa))
      expect(host.querySelector("[data-mcp-group]")).toBeNull()
      expect(button("Edit", row("charts"))?.disabled).toBe(false)
      expect(document.activeElement).toBe(button("Remove", row("charts")))
    })
  })

  describe("the form's fields (F11, F12)", () => {
    async function adding() {
      const fake = fakeGateway()
      await mount(fake.gateway)
      await answer(fake, "list", list(charts, nessa))
      await click(button("Add server…"))
      return fake
    }
    const fields = () =>
      [
        ...form().querySelectorAll("[data-mcp-argument] textarea"),
      ] as HTMLTextAreaElement[]
    async function key(element: Element | null, init: KeyboardEventInit) {
      if (!element) throw new Error("nothing to press on")
      const event = new KeyboardEvent("keydown", {
        bubbles: true,
        cancelable: true,
        ...init,
      })
      await act(async () => {
        element.dispatchEvent(event)
      })
      return event
    }

    it("F11 and F12: Add argument focuses the new field; Enter adds the next after it, focused", async () => {
      await adding()
      await click(button("Add argument", form()))
      expect(document.activeElement).toBe(fields()[0])
      await type(fields()[0], "a")
      await click(button("Add argument", form()))
      await type(fields()[1], "c")
      const enter = await key(fields()[0], { key: "Enter" })
      expect(enter.defaultPrevented).toBe(true)
      expect(fields().map((each) => each.value)).toEqual(["a", "", "c"])
      expect(document.activeElement).toBe(fields()[1])
    })

    it("F11: Shift+Enter is a line break, marked under its field", async () => {
      await adding()
      await click(button("Add argument", form()))
      const shifted = await key(fields()[0], { key: "Enter", shiftKey: true })
      expect(shifted.defaultPrevented).toBe(false)
      expect(fields()).toHaveLength(1)
      await type(fields()[0], "a\nb")
      const marker = form().querySelector("[data-mcp-argument-breaks]")
      expect(marker?.textContent).toBe("Has a line break")
      expect(fields()[0].getAttribute("aria-describedby")).toContain(marker?.id)
      await type(fields()[0], "a\r\nb\nc")
      expect(form().querySelector("[data-mcp-argument-breaks]")?.textContent).toBe(
        "Has 2 line breaks",
      )
      await type(fields()[0], "ab")
      expect(form().querySelector("[data-mcp-argument-breaks]")).toBeNull()
    })

    it("F11: Enter while composing is the input method's", async () => {
      await adding()
      await click(button("Add argument", form()))
      const event = new KeyboardEvent("keydown", {
        key: "Enter",
        bubbles: true,
        cancelable: true,
        isComposing: true,
      })
      await act(async () => {
        fields()[0].dispatchEvent(event)
      })
      expect(fields()).toHaveLength(1)
    })

    it("F11 (M1): Enter that Safari marks only by keyCode 229 is the input method's", async () => {
      await adding()
      await click(button("Add argument", form()))
      const event = new KeyboardEvent("keydown", {
        key: "Enter",
        bubbles: true,
        cancelable: true,
      })
      Object.defineProperty(event, "keyCode", { value: 229 })
      await act(async () => {
        fields()[0].dispatchEvent(event)
      })
      expect(event.defaultPrevented).toBe(false)
      expect(fields()).toHaveLength(1)
    })

    it("F12: removing an argument focuses the next one, or Add argument", async () => {
      await adding()
      for (const value of ["a", "b"]) {
        await click(button("Add argument", form()))
        await type(fields().at(-1) ?? null, value)
      }
      await click(form().querySelector('[aria-label="Remove argument 1"]') ?? undefined)
      expect(fields().map((each) => each.value)).toEqual(["b"])
      expect(document.activeElement).toBe(fields()[0])
      await click(form().querySelector('[aria-label="Remove argument 1"]') ?? undefined)
      expect(document.activeElement).toBe(button("Add argument", form()))
    })

    it("F12: Add variable focuses its name; removing one focuses the next, or Add variable", async () => {
      await adding()
      const names = () =>
        [...form().querySelectorAll('[aria-label="Variable name"]')] as HTMLInputElement[]
      await click(button("Add variable", form()))
      expect(document.activeElement).toBe(names()[0])
      await type(names()[0], "A")
      await click(button("Add variable", form()))
      expect(document.activeElement).toBe(names()[1])
      await type(names()[1], "B")
      await click(
        button("Remove", form().querySelector('[data-mcp-variable="A"]') ?? undefined),
      )
      expect(document.activeElement).toBe(names()[0])
      expect(names()[0].value).toBe("B")
      await click(
        button("Remove", form().querySelector('[data-mcp-variable="B"]') ?? undefined),
      )
      expect(document.activeElement).toBe(button("Add variable", form()))
    })
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
      await answer(fake, "save", { ok: true, value: { live: true } })
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
      await answer(fake, "save", { ok: true, value: { live: true } })
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
      await answer(fake, "remove", { ok: true, value: { live: true } })
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

  it("a remote row shows its consent state and sends authorize and revoke", async () => {
    const docs: ListedServer = {
      name: "docs",
      command: "",
      args: [],
      envNames: [],
      enabled: true,
      managed: false,
      url: "https://mcp.example/mcp",
      remoteId: "11111111-1111-4111-8111-111111111111",
      authorization: {
        phase: "consent_needed",
        tokenExpired: false,
        refreshFailing: false,
        scopeRequired: false,
      },
    }
    const fake = fakeGateway()
    await mount(fake.gateway)
    await answer(fake, "list", list(docs, nessa))
    const docsRow = row("docs")
    expect(docsRow.textContent).toContain("https://mcp.example/mcp")
    expect(docsRow.textContent).toContain("Consent needed")
    expect(button("Authorize", docsRow)).toBeDefined()
    expect(button("Revoke", docsRow)).toBeDefined()
    expect(button("Authorize", row("nessa"))).toBeUndefined()
    await click(button("Authorize", docsRow))
    const authorize = fake.requests.filter((each) => each.method === "authorize").at(-1)
    expect(authorize?.argument).toEqual({ id: docs.remoteId, revision: "r1" })
    await answer(fake, "authorize", {
      ok: true,
      value: { status: "pending_consent", consentUrl: "http://127.0.0.1:9/start" },
    })
    const consent = host.querySelector("[data-mcp-consent]")
    expect(consent?.getAttribute("href")).toBe("http://127.0.0.1:9/start")
    expect(consent?.textContent).toBe("Waiting for consent")
    await click(button("Revoke", row("docs")))
    const revoke = fake.requests.filter((each) => each.method === "revoke").at(-1)
    expect(revoke?.argument).toEqual({ id: docs.remoteId, revision: "r1" })
  })
})
