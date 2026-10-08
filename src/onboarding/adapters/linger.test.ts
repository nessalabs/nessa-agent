import { describe, expect, it } from "vitest"

import { hostLinger } from "./linger"

describe("hostLinger", () => {
  it("parses the browser no-op as not applicable", async () => {
    expect(await hostLinger.status()).toEqual({
      shown: "not-applicable",
      audit: "not-required",
    })
    expect(await hostLinger.accept()).toEqual({
      shown: "not-applicable",
      audit: "not-required",
    })
    expect(await hostLinger.decline()).toEqual({
      shown: "not-applicable",
      audit: "not-required",
    })
  })
})
