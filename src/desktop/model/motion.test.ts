import { expect, it } from "vitest"
import { motionInEffect, parseMotionChoice } from "./motion"

it("follows the system only where the person left it to it", () => {
  expect(motionInEffect("system", true)).toBe("reduced")
  expect(motionInEffect("system", false)).toBe("full")
  expect(motionInEffect("full", true)).toBe("full")
  expect(motionInEffect("reduced", false)).toBe("reduced")
})

it("reads anything it does not know as following the system", () => {
  expect(parseMotionChoice("reduced")).toBe("reduced")
  expect(parseMotionChoice("constructor")).toBe("system")
  expect(parseMotionChoice(null)).toBe("system")
})
