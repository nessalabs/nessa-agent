// @vitest-environment jsdom
/**
 * Settings › Access remembered rules: the row rests without a gateway, and
 * with one it lists the rules and revokes the one the person chooses.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { PermissionRule } from "@nessa/client"
import type { PermissionRulesGateway } from "../adapters/permission-rules-gateway"
import {
  PermissionRuleRows,
  PermissionRulesProvider,
  TrustedAppRows,
} from "./permission-rule-rows"

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
  vi.restoreAllMocks()
})

const rule: PermissionRule = {
  id: "pr_one",
  revision: "1",
  workspace: "/work",
  source: "claude",
  tool: "Bash",
  effect: "allow",
}

it("rests when this window has no gateway", async () => {
  await act(async () => root.render(<PermissionRuleRows />))
  expect(host.textContent).toContain("Not available in this window.")
})

it("lists a rule and revokes it", async () => {
  let rules = [rule]
  const revoked: string[] = []
  const gateway: PermissionRulesGateway = {
    read: async () => ({ ok: true, rules }),
    revoke: async (ruleId) => {
      revoked.push(ruleId)
      rules = []
      return { ok: true, applied: true }
    },
  }
  await act(async () =>
    root.render(
      <PermissionRulesProvider gateway={gateway}>
        <PermissionRuleRows />
      </PermissionRulesProvider>,
    ),
  )
  expect(host.textContent).toContain("Always allow Bash")
  expect(host.textContent).toContain("/work")
  const button = host.querySelector("button")
  await act(async () => button?.click())
  expect(revoked).toEqual(["pr_one"])
  expect(host.textContent).toContain("Rules you save from a review appear here.")
})

it("says a failed revoke left the rule stored only when the reread still lists it", async () => {
  const gateway: PermissionRulesGateway = {
    read: async () => ({ ok: true, rules: [rule] }),
    revoke: async () => ({ ok: false }),
  }
  await show(gateway)
  await act(async () => host.querySelector("button")?.click())
  expect(host.textContent).toContain("Could not revoke that rule.")
  expect(host.textContent).toContain("It is still remembered.")
  expect(host.textContent).toContain("Always allow Bash")
})

it("does not say a failed revoke left the rule stored when the reread omits it", async () => {
  let rules = [rule]
  const gateway: PermissionRulesGateway = {
    read: async () => ({ ok: true, rules }),
    revoke: async () => {
      rules = []
      return { ok: false }
    },
  }
  await show(gateway)
  await act(async () => host.querySelector("button")?.click())
  expect(host.textContent).toContain("Could not revoke that rule.")
  expect(host.textContent).toContain("It is no longer listed.")
  expect(host.textContent).not.toContain("It is still remembered.")
  expect(host.textContent).not.toContain("Always allow Bash")
})

it("says the list could not be read again when a failed revoke cannot be reread", async () => {
  let reads = 0
  const gateway: PermissionRulesGateway = {
    read: async () => {
      reads += 1
      return reads === 1 ? { ok: true, rules: [rule] } : { ok: false }
    },
    revoke: async () => ({ ok: false }),
  }
  await show(gateway)
  await act(async () => host.querySelector("button")?.click())
  expect(host.textContent).toContain("Could not revoke that rule.")
  expect(host.textContent).toContain("The list could not be read again.")
  expect(host.textContent).toContain("Always allow Bash")
})

async function show(gateway: PermissionRulesGateway) {
  await act(async () =>
    root.render(
      <PermissionRulesProvider gateway={gateway}>
        <PermissionRuleRows />
      </PermissionRulesProvider>,
    ),
  )
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
function revokeButton() {
  const button = host.querySelector<HTMLButtonElement>("button")
  if (!button) throw new Error("Missing Revoke button")
  return button
}

it.each(["focus", "visibilitychange"])(
  "retains revoke ownership through a %s refresh",
  async (event) => {
    vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible")
    const pending = deferred<Awaited<ReturnType<PermissionRulesGateway["revoke"]>>>()
    let calls = 0,
      reads = 0
    const gateway: PermissionRulesGateway = {
      read: async () => {
        reads++
        return { ok: true, rules: [rule] }
      },
      revoke: () => {
        calls++
        return pending.promise
      },
    }
    await show(gateway)
    await act(async () => revokeButton().click())
    expect(revokeButton().disabled).toBe(true)
    await act(async () => {
      if (event === "focus") window.dispatchEvent(new Event(event))
      else document.dispatchEvent(new Event(event))
    })
    expect(revokeButton().disabled).toBe(true)
    expect(reads).toBe(1)
    expect(calls).toBe(1)
    await act(async () => pending.resolve({ ok: false }))
    expect(reads).toBe(2)
    expect(host.textContent).toContain("Could not revoke that rule.")
    expect(revokeButton().disabled).toBe(false)
  },
)

it("retains revoke ownership until its post-command read finishes", async () => {
  const pending = deferred<Awaited<ReturnType<PermissionRulesGateway["read"]>>>()
  let reads = 0
  const gateway: PermissionRulesGateway = {
    read: async () => (++reads === 1 ? { ok: true, rules: [rule] } : pending.promise),
    revoke: async () => ({ ok: true, applied: true }),
  }
  await show(gateway)
  await act(async () => revokeButton().click())
  await act(async () => window.dispatchEvent(new Event("focus")))
  expect(reads).toBe(2)
  expect(revokeButton().disabled).toBe(true)
  await act(async () => pending.resolve({ ok: true, rules: [] }))
  expect(host.textContent).toContain("Rules you save from a review appear here.")
})

it("ignores a background read begun before a revoke", async () => {
  const oldRead = deferred<Awaited<ReturnType<PermissionRulesGateway["read"]>>>()
  const pending = deferred<Awaited<ReturnType<PermissionRulesGateway["revoke"]>>>()
  let reads = 0
  const gateway: PermissionRulesGateway = {
    read: async () => (++reads === 2 ? oldRead.promise : { ok: true, rules: [rule] }),
    revoke: () => pending.promise,
  }
  await show(gateway)
  await act(async () => window.dispatchEvent(new Event("focus")))
  await act(async () => revokeButton().click())
  await act(async () => oldRead.resolve({ ok: true, rules: [rule] }))
  expect(revokeButton().disabled).toBe(true)
  await act(async () => pending.resolve({ ok: false }))
  expect(host.textContent).toContain("Could not revoke that rule.")
})

it.each(["replace", "unmount"])(
  "retires a revoke before an obsolete follow-up read on %s",
  async (retirement) => {
    const pending = deferred<Awaited<ReturnType<PermissionRulesGateway["revoke"]>>>()
    let reads = 0
    const gateway: PermissionRulesGateway = {
      read: async () => {
        reads++
        return { ok: true, rules: [rule] }
      },
      revoke: () => pending.promise,
    }
    await show(gateway)
    await act(async () => revokeButton().click())
    if (retirement === "replace")
      await show({
        read: async () => ({ ok: true, rules: [] }),
        revoke: async () => ({ ok: true, applied: true }),
      })
    else await act(async () => root.render(null))
    await act(async () => pending.resolve({ ok: true, applied: true }))
    expect(reads).toBe(1)
    expect(host.textContent).not.toContain("Always allow Bash")
  },
)

it("admits one revoke when two clicks precede the disabled render", async () => {
  const pending = deferred<Awaited<ReturnType<PermissionRulesGateway["revoke"]>>>()
  let calls = 0
  await show({
    read: async () => ({ ok: true, rules: [rule] }),
    revoke: () => {
      calls++
      return pending.promise
    },
  })
  const button = revokeButton()
  await act(async () => {
    button.click()
    button.click()
  })
  expect(calls).toBe(1)
  await act(async () => pending.resolve({ ok: false }))
})

const ruleIn = (id: string, workspace: string, tool: string): PermissionRule => ({
  ...rule,
  id,
  workspace,
  tool,
})

it("folds rules by project, each saying how many it holds", async () => {
  await show({
    read: async () => ({
      ok: true,
      rules: [
        ruleIn("a", "/work/site", "Bash"),
        ruleIn("b", "/work/site", "WebSearch"),
        ruleIn("c", "/home/me/api", "Read"),
      ],
    }),
    revoke: async () => ({ ok: true, applied: true }),
  })
  const projects = [...host.querySelectorAll("[data-rule-project]")]
  expect(projects.map((row) => row.getAttribute("data-rule-project"))).toEqual([
    "/home/me/api",
    "/work/site",
  ])
  expect(host.textContent).toContain("2 rules · /work/site")
  expect(host.textContent).toContain("1 rule · /home/me/api")
  // Folded: no rule shows until its project is opened.
  expect(host.querySelector("[data-rule]")).toBeNull()
  const site = host.querySelector<HTMLButtonElement>(
    '[data-rule-project="/work/site"] button[aria-expanded]',
  )
  await act(async () => site?.click())
  expect(site?.getAttribute("aria-expanded")).toBe("true")
  expect(
    [...host.querySelectorAll("[data-rule]")].map((row) => row.getAttribute("data-rule")),
  ).toEqual(["a", "b"])
})

it("searches every project once the list is long", async () => {
  const rules = Array.from({ length: 8 }, (_, index) =>
    ruleIn(`r${index}`, `/work/p${index % 3}`, index === 5 ? "WebFetch" : `Tool${index}`),
  )
  await show({
    read: async () => ({ ok: true, rules }),
    revoke: async () => ({ ok: true, applied: true }),
  })
  const search = host.querySelector<HTMLInputElement>(
    "[data-rule-search] input, input[data-rule-search]",
  )
  expect(search).not.toBeNull()
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set
    set?.call(search, "webfetch")
    search?.dispatchEvent(new Event("input", { bubbles: true }))
  })
  expect(
    [...host.querySelectorAll("[data-rule]")].map((row) => row.getAttribute("data-rule")),
  ).toEqual(["r5"])
  expect(host.querySelectorAll("[data-rule-project]")).toHaveLength(1)
})

it("lists the apps always allowed and revokes one", async () => {
  let servers = ["charts", "huddle"]
  const untrusted: string[] = []
  await act(async () =>
    root.render(
      <PermissionRulesProvider
        gateway={{
          read: async () => ({ ok: true, rules: [] }),
          revoke: async () => ({ ok: true, applied: true }),
          trustedApps: {
            read: async () => ({ ok: true, servers }),
            untrust: async (server) => {
              untrusted.push(server)
              servers = servers.filter((each) => each !== server)
              return { ok: true, applied: true }
            },
          },
        }}
      >
        <TrustedAppRows />
      </PermissionRulesProvider>,
    ),
  )
  expect(host.textContent).toContain("Apps you always allow")
  expect(
    [...host.querySelectorAll("[data-trusted-app]")].map((row) =>
      row.getAttribute("data-trusted-app"),
    ),
  ).toEqual(["charts", "huddle"])
  await act(async () =>
    host.querySelector<HTMLButtonElement>('button[aria-label="Revoke huddle"]')?.click(),
  )
  expect(untrusted).toEqual(["huddle"])
  expect(
    [...host.querySelectorAll("[data-trusted-app]")].map((row) =>
      row.getAttribute("data-trusted-app"),
    ),
  ).toEqual(["charts"])
})

it("draws no apps group where the window cannot list apps", async () => {
  await show({
    read: async () => ({ ok: true, rules: [] }),
    revoke: async () => ({ ok: true, applied: true }),
  })
  await act(async () => root.render(<TrustedAppRows />))
  expect(host.textContent).toBe("")
})
