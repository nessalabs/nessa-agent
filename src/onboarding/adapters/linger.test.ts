import { describe, expect, it } from "vitest"

import { hostLinger } from "./linger"

describe("hostLinger", () => {
  it("is the no-op view in a browser", async () => {
    expect(await hostLinger.status()).toEqual({ shown: "not-applicable" })
    expect(await hostLinger.accept()).toEqual({ shown: "not-applicable" })
  })
})
