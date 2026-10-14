// @vitest-environment jsdom
/**
 * Settings › Access remembered rules: the row rests without a gateway, and
 * with one it lists the rules and revokes the one the person chooses.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
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
