import { describe, expect, it } from "vitest"
import { startupDetails, startupSentence } from "./copy"
import type { GatewayStartupStatus } from "./gateway-startup"

describe("what a person reads about startup (ADR 221)", () => {
  const every: [GatewayStartupStatus, string | undefined][] = [
    [{ revision: 1, state: "starting", step: "preparing" }, "Getting ready…"],
    [{ revision: 1, state: "starting", step: "replacing" }, "Finishing the last update…"],
    [{ revision: 1, state: "starting", step: "launching" }, "Starting…"],
    [{ revision: 1, state: "failed", message: "launchctl bootstrap: 5" }, "Nessa couldn’t start."],
    [
      { state: "unavailable", message: "no host" },
      "Nessa couldn’t check whether it started.",
    ],
    [{ revision: 1, state: "ready" }, undefined],
    [{ revision: 1, state: "unmanaged" }, undefined],
  ]

  it.each(every)("says one plain sentence for %j", (status, sentence) => {
    expect(startupSentence(status)).toBe(sentence)
  })

  it("never shows the host's technical words, or the word gateway, as the sentence", () => {
    for (const [status] of every) {
      const sentence = startupSentence(status) ?? ""
      expect(sentence.toLowerCase()).not.toContain("gateway")
      expect(sentence).not.toContain("launchctl")
      expect(sentence).not.toContain("no host")
    }
  })

  it("reads a step it does not know as a start in progress", () => {
    const unknown = { revision: 1, state: "starting", step: "defragmenting" } as unknown
    expect(startupSentence(unknown as GatewayStartupStatus)).toBe("Getting ready…")
  })

  it("keeps the host's message for Details only when something went wrong", () => {
    expect(startupDetails({ revision: 1, state: "failed", message: "why" })).toBe("why")
    expect(startupDetails({ state: "unavailable", message: "how" })).toBe("how")
    expect(
      startupDetails({ revision: 1, state: "starting", step: "launching" }),
    ).toBeUndefined()
  })
})
