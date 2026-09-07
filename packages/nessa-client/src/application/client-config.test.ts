import { expect, it } from "vitest"
import { NessaClientConfig } from "./client-config.js"

it("provides independent immutable defaults and copies caller settings", () => {
  const input = { retry: { maxAttempts: 5 }, requestTimeoutMs: 1234 }
  const customized = new NessaClientConfig(input)
  const defaults = new NessaClientConfig()
  input.retry.maxAttempts = 100
  expect(customized.retry.maxAttempts).toBe(5)
  expect(defaults.retry).toEqual({
    maxAttempts: 3,
    initialDelayMs: 250,
    maxDelayMs: 2000,
    jitter: true,
  })
  expect(defaults.requestTimeoutMs).toBe(30_000)
  expect(customized.requestTimeoutMs).toBe(1234)
  expect(Object.isFrozen(customized)).toBe(true)
  expect(Object.isFrozen(customized.retry)).toBe(true)
})

it.each([0, -1, NaN, Infinity, 0.5, 2_147_483_648])(
  "rejects invalid RPC timeout %s",
  (requestTimeoutMs) => {
    expect(() => new NessaClientConfig({ requestTimeoutMs })).toThrow("requestTimeoutMs")
  },
)
