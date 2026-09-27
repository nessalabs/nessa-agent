import { describe, expect, it } from "vitest"
import { greetingFor } from "./greeting"

describe("greetingFor", () => {
  it.each([
    [4, "Working late"],
    [5, "Good morning"],
    [11, "Good morning"],
    [12, "Good afternoon"],
    [16, "Good afternoon"],
    [17, "Good evening"],
    [21, "Good evening"],
    [22, "Working late"],
    [0, "Working late"],
  ])("greets hour %i with %s", (hour, greeting) => {
    expect(greetingFor(hour)).toBe(greeting)
  })
})
