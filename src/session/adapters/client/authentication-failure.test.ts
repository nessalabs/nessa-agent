import { expect, it } from "vitest"
import { NessaConnectionClosedError, NessaRpcError } from "@nessa/client"
import { isAuthenticationFailure } from "./authentication-failure"
it("separates authentication termination from transport and protocol failures", () => {
  for (const code of [4001, 4002, 4003, 4004])
    expect(isAuthenticationFailure(new NessaConnectionClosedError(code, ""))).toBe(true)
  for (const code of [1006, 1012, 4005, 4010])
    expect(isAuthenticationFailure(new NessaConnectionClosedError(code, ""))).toBe(false)
  expect(isAuthenticationFailure(new NessaRpcError("unauthorized", ""))).toBe(true)
  expect(isAuthenticationFailure(new NessaRpcError("temporarily_unavailable", ""))).toBe(
    false,
  )
})
