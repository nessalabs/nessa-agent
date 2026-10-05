import { describe, expect, it } from "vitest"
import { COULD_NOT_START } from "../startup/application/copy"
import {
  documentUnservedSentence,
  hostRefusalFromInvoke,
  HostRefusalError,
  scriptUnservedSentence,
  startupCode,
  startupLine,
  startupRefusalSentence,
  wrongStageSentence,
  type StartupCodeKey,
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

  it("publishes one line and a code for every startup failure", () => {
    expect(startupLine()).toBe(COULD_NOT_START)
    expect(startupLine()).toBe("Nessa couldn’t start")
    const codes: Record<StartupCodeKey, string> = {
      "not-provisioned": "STARTUP_GATEWAY",
      "not-ready": "STARTUP_GATEWAY",
      "not-listening": "STARTUP_GATEWAY",
      "wrong-stage": "STARTUP_STAGE",
      "document-unserved": "STARTUP_PAGE",
      "script-unserved": "STARTUP_MODULE",
      "still-compiling": "STARTUP_COMPILE",
      runtime: "STARTUP_PAGE",
      host: "STARTUP_HOST",
    }
    for (const [key, code] of Object.entries(codes) as [StartupCodeKey, string][])
      expect(startupCode(key)).toBe(code)
  })

  it("fills the page and the script into the dev-server sentences", () => {
    const page = "http://localhost:1420/desktop.html"
    const script = "/src/desktop/main.tsx"
    expect(documentUnservedSentence(page)).toContain(page)
    expect(scriptUnservedSentence(page, script)).toContain(script)
    expect(scriptUnservedSentence(page, script)).toContain("did not serve")
  })
})
