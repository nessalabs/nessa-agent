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
