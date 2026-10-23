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
import { PermissionRuleRows, PermissionRulesProvider } from "./permission-rule-rows"

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
