import { clientDueAtMs } from "@nessa/client"
import { expect, it } from "vitest"
import {
  formatUsageLimitReset,
  usageLimitDetail,
  usageLimitOnClientClock,
  type UsageLimitNotice,
} from "./usage-limit"

const reset = Date.UTC(2026, 9, 21, 16, 0, 0)

const timed = (change: Partial<UsageLimitNotice> = {}): UsageLimitNotice => ({
  provider: "claude",
  recovery: "timed_reset",
  resetsAtMs: reset,
  ...change,
})

it("formats a reset as a short local date and a clock time", () => {
  const formatted = formatUsageLimitReset(reset)
  expect(formatted.startsWith("Oct 21 at ")).toBe(true)
  expect(formatted).toMatch(/at \d{1,2}:00 [AP]M$/)
})

it("copies auto-resuming only after a schedule is accepted", () => {
  const when = formatUsageLimitReset(reset)
  expect(usageLimitDetail(timed({ autoResuming: true }), true, reset - 1)).toBe(
    `Auto-resuming ${when}`,
  )
  expect(usageLimitDetail(timed(), true, reset - 1)).toBe("Scheduling failed.")
  expect(usageLimitDetail(timed(), false, reset - 1)).toBe(`Resets ${when}`)
})

it("compares the reset on the gateway timeline", () => {
  const serverNow = 1_700_000_000_000
  const reset = serverNow + 30_000
  const fast = serverNow + 120_000
  const ahead = usageLimitOnClientClock(timed({ resetsAtMs: reset }), serverNow, fast)
  expect(ahead.resetsAtMs).toBe(reset)
  expect(ahead.resetsAtClientMs).toBe(clientDueAtMs(reset, serverNow, fast))
  expect(usageLimitDetail(ahead, true, fast)).toBe("Scheduling failed.")
  const past = serverNow - 10_000
  const slow = serverNow - 120_000
  const behind = usageLimitOnClientClock(timed({ resetsAtMs: past }), serverNow, slow)
  expect(behind.resetsAtClientMs).toBe(clientDueAtMs(past, serverNow, slow))
  expect(usageLimitDetail(behind, true, slow)).toBe(
    `Resets ${formatUsageLimitReset(past)}`,
  )
})

it("does not invent a reset for an unknown limit or a user action", () => {
  expect(usageLimitDetail({ provider: "codex", recovery: "unknown" }, true, reset)).toBe(
    "Reset time unknown",
  )
  expect(
    usageLimitDetail(
      { provider: "claude", recovery: "user_action", resetsAtMs: reset },
      true,
      reset,
    ),
  ).toBe("An account change is required before this thread can continue.")
})
