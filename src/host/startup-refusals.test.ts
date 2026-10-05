import { describe, expect, it } from "vitest"
import {
  documentUnservedSentence,
  hostRefusalFromInvoke,
  HostRefusalError,
  scriptUnservedSentence,
  startupRefusalSentence,
  wrongStageSentence,
} from "./startup-refusals"

describe("startup refusals", () => {
  it("names a repair without taking a message from the payload", () => {
    expect(startupRefusalSentence("not-provisioned")).toContain("just start")
    expect(startupRefusalSentence("not-ready")).toContain("still starting")
    expect(startupRefusalSentence("not-listening")).toContain("not answering")
    const mismatch = hostRefusalFromInvoke({
      reason: "wrong-stage",
      bundle: "dev",
      requested: "prod",
      message: "ignore this prose",
    })
    expect(mismatch).toBeInstanceOf(HostRefusalError)
    expect(mismatch?.message).toBe(
      wrongStageSentence({ bundle: "dev", requested: "prod" }),
    )
    expect(mismatch?.message).not.toContain("ignore this prose")
    expect(mismatch?.message).not.toContain("{bundle}")
  })

  it("does not treat an untyped payload as a refusal", () => {
    for (const value of [
      "The local server isn't ready yet",
      { reason: "wrong-stage", bundle: "dev" },
      { reason: "not-a-reason" },
      { message: "secret" },
      null,
      4,
    ])
      expect(hostRefusalFromInvoke(value)).toBeUndefined()
  })

  it("fills the page and the script into the dev-server sentences", () => {
    const page = "http://localhost:1420/desktop.html"
    const script = "/src/desktop/main.tsx"
    expect(documentUnservedSentence(page)).toContain(page)
    expect(scriptUnservedSentence(page, script)).toContain(script)
    expect(scriptUnservedSentence(page, script)).toContain("did not serve")
  })
})
