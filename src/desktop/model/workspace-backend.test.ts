import { expect, it } from "vitest"
import { gatewayRequested } from "./workspace-backend"

it("asks for the gateway only when the address names it", () => {
  expect(gatewayRequested("?gateway")).toBe(true)
  expect(gatewayRequested("?theme=dark&gateway=1")).toBe(true)
  expect(gatewayRequested("")).toBe(false)
  expect(gatewayRequested("?gateways")).toBe(false)
  expect(gatewayRequested("?theme=gateway")).toBe(false)
})
