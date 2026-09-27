import { describe, expect, it } from "vitest"
import { gatewayOrigin, gatewayPort, parseStage, type Stage } from "./gateway-ports"

const stages: Stage[] = ["dev", "ci", "alpha", "prod"]

/**
 * Names no table put in itself, which every object answers to anyway. `in`
 * found all of them and a plain read returned them, so each one used to arrive
 * at `gatewayOrigin` as a port.
 */
const inherited = [
  "toString",
  "constructor",
  "valueOf",
  "hasOwnProperty",
  "isPrototypeOf",
  "propertyIsEnumerable",
  "toLocaleString",
  "__proto__",
]

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

  /**
   * `Stage` is only as good as the narrowing that made it, and a cast makes
   * none — which is exactly how the Vite proxy reached this function before
   * `parseStage` refused these names. A port is a number; an inherited member
   * is not, and the old `port === undefined` test could not tell the
   * difference.
   */
  it("refuses a name every object answers to, cast or not", () => {
    for (const named of inherited) {
      expect(() => gatewayPort(named as Stage)).toThrow(/No gateway port/)
      expect(() => gatewayOrigin(named as Stage)).toThrow(/No gateway port/)
    }
  })
})

describe("reading NESSA_STAGE", () => {
  it("is the dev stage when the variable says nothing", () => {
    expect(parseStage(undefined)).toBe("dev")
    expect(parseStage("")).toBe("dev")
    expect(parseStage("   ")).toBe("dev")
  })

  it("names every stage the table names, whitespace and all", () => {
    for (const stage of stages) expect(parseStage(stage)).toBe(stage)
    expect(parseStage(" ci ")).toBe("ci")
  })

  it("refuses a stage the server would refuse, in the same words", () => {
    expect(() => parseStage("Dev")).toThrow(/is not a stage/)
    expect(() => parseStage("staging")).toThrow(/dev, ci, alpha, prod/)
  })

  /**
   * The bug this file is the flagship of. `named in stages` walks the
   * prototype chain, so the guard did not fire; `stages.toString` is a
   * function, so the `undefined` check downstream did not fire either; and
   * `NESSA_STAGE=toString` built
   * `http://127.0.0.1:function toString() { [native code] }`.
   */
  it("refuses a name every object answers to", () => {
    for (const named of inherited) {
      expect(() => parseStage(named)).toThrow(/is not a stage/)
      expect(() => parseStage(` ${named} `)).toThrow(/is not a stage/)
    }
  })
})
