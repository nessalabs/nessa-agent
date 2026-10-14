import { describe, expect, it } from "vitest"
import { createQuestionFlights, differentFlight } from "./question-flights"

describe("question flights", () => {
  it("lets a second caller hear the refusal, then start again", async () => {
    const flights = createQuestionFlights()
    let release: (outcome: "sent" | "refused") => void = () => {}
    const start = () =>
      new Promise<"sent" | "refused">((resolve) => {
        release = resolve
      })
    let started = 0
    const first = flights.join("ask", "skip", () => {
      started += 1
      return start()
    })
    const second = flights.join("ask", "skip", () => {
      started += 1
      return start()
    })
    expect(started).toBe(1)
    release("refused")
    expect(await first).toBe("refused")
    expect(await second).toBe("refused")
    const again = flights.join("ask", "skip", async () => {
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
      "skip",
      () =>
        new Promise<void>((resolve) => {
          release = resolve
        }),
    )
    expect(await other.join("ask", "skip", async () => "other")).toBe("other")
    release()
    await held
  })

  it("does not hand a different answer the outcome already on its way", async () => {
    const flights = createQuestionFlights()
    let release: (outcome: "sent") => void = () => {}
    let started = 0
    const first = flights.join("ask", "skip", () => {
      started += 1
      return new Promise<"sent">((resolve) => {
        release = resolve
      })
    })
    const different = flights.join("ask", "answer", async () => {
      started += 1
      return "sent" as const
    })
    let settled = false
    void different.then(() => {
      settled = true
    })
    await Promise.resolve()
    expect(settled).toBe(true)
    expect(started).toBe(1)
    expect(await different).toBe(differentFlight)
    release("sent")
    expect(await first).toBe("sent")
    const after = flights.join("ask", "answer", async () => {
      started += 1
      return "sent" as const
    })
    expect(await after).toBe("sent")
    expect(started).toBe(2)
  })
})
