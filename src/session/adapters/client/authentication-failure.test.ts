import { expect, it } from "vitest"
import {
  NessaConnectionClosedError,
  NessaCredentialUnavailableError,
  NessaRpcError,
} from "@nessa/client"
import { isAuthenticationFailure, isSignedOut } from "./authentication-failure"
import { SessionHealthError } from "./dev-session"
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

it("is signed out with no credential, or one the gateway refused, at the handshake or the probe", () => {
  const refused = new NessaRpcError("unauthorized", "")
  expect(isSignedOut(new NessaCredentialUnavailableError())).toBe(true)
  expect(isSignedOut(refused)).toBe(true)
  expect(isSignedOut(new NessaConnectionClosedError(4001, ""))).toBe(true)
  expect(isSignedOut(new SessionHealthError("probe", refused))).toBe(true)
})

it("is not signed out for no answer, another refusal, or a host's sentence", () => {
  expect(isSignedOut(new NessaConnectionClosedError(1006, ""))).toBe(false)
  expect(isSignedOut(new NessaRpcError("temporarily_unavailable", ""))).toBe(false)
  expect(
    isSignedOut(
      new SessionHealthError("probe", new NessaConnectionClosedError(1006, "")),
    ),
  ).toBe(false)
  // The host's refusals cross IPC as sentences (`loadAssignedSurfaceCredential`):
  // which repair they need is not this rule's to guess from the words.
  expect(isSignedOut(new Error("No local surface credential is available."))).toBe(false)
})
