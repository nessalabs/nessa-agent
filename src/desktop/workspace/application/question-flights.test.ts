import { describe, expect, it } from "vitest"
import { createQuestionFlights } from "./question-flights"

describe("question flights", () => {
  it("lets a second caller hear the refusal, then start again", async () => {
    const flights = createQuestionFlights()
    let release: (outcome: "sent" | "refused") => void = () => {}
    const start = () =>
      new Promise<"sent" | "refused">((resolve) => {
        release = resolve
      })
    let started = 0
    const first = flights.join("ask", () => {
      started += 1
      return start()
    })
    const second = flights.join("ask", () => {
      started += 1
      return start()
    })
    expect(started).toBe(1)
    release("refused")
    expect(await first).toBe("refused")
    expect(await second).toBe("refused")
    const again = flights.join("ask", async () => {
      started += 1
      return "sent" as const
    })
    expect(await again).toBe("sent")
    expect(started).toBe(2)
  })

  it("keeps two composed records apart", async () => {
    const one = createQuestionFlights()
    const other = createQuestionFlights()
    let release: () => void = () => {}
    const held = one.join(
      "ask",
      () =>
        new Promise<void>((resolve) => {
          release = resolve
        }),
    )
    expect(await other.join("ask", async () => "other")).toBe("other")
    release()
    await held
  })
})
