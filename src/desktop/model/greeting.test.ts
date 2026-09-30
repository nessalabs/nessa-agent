import { describe, expect, it } from "vitest"
import { greetingAt } from "./greeting"

describe("the home's greeting", () => {
  it("follows the hour of the day, with its edges where they are said to be", () => {
    expect(greetingAt(4)).toBe("Working late?")
    expect(greetingAt(5)).toBe("Good morning")
    expect(greetingAt(11)).toBe("Good morning")
    expect(greetingAt(12)).toBe("Good afternoon")
    expect(greetingAt(16)).toBe("Good afternoon")
    expect(greetingAt(17)).toBe("Good evening")
    expect(greetingAt(21)).toBe("Good evening")
    expect(greetingAt(22)).toBe("Working late?")
    expect(greetingAt(0)).toBe("Working late?")
  })
})
