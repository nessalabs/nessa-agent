import { describe, expect, it } from "vitest"
import { gatewayOrigin, gatewayPort, type Stage } from "./gateway-ports"

const stages: Stage[] = ["dev", "ci", "alpha", "prod"]

describe("gateway ports", () => {
  it("has a port for every stage the server accepts", () => {
    for (const stage of stages) expect(gatewayPort(stage)).toBeGreaterThan(0)
  })

  it("keeps the product port for prod and moves dev beside it", () => {
    expect(gatewayPort("prod")).toBe(7420)
    expect(gatewayPort("dev")).toBe(7421)
  })

  it("renders an origin with no trailing slash", () => {
    expect(gatewayOrigin("dev")).toBe("http://127.0.0.1:7421")
  })

  it("refuses a stage the table does not name", () => {
    expect(() => gatewayPort("staging" as Stage)).toThrow()
  })
})
