// @vitest-environment jsdom
/**
 * The checkbox shows a choice that has not landed. The line under the title
 * follows the confirmed preference, so that interval does not say the
 * schedule failed.
 */
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"

import { formatUsageLimitReset, usageLimitDetail } from "../model/usage-limit"
import { UsageLimitCard } from "./usage-limit-card"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const reset = Date.UTC(2026, 9, 21, 16, 0, 0)
const limit = {
  provider: "claude" as const,
  recovery: "timed_reset" as const,
  resetsAtMs: reset,
}

let host: HTMLDivElement
let root: Root

beforeEach(() => {
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(() => {
  act(() => root.unmount())
  host.remove()
})

function detail(): string {
  return host.querySelector("[data-usage-limit-detail]")?.textContent ?? ""
}

it("keeps the reset line while an enable is still pending", async () => {
  let release: () => void = () => {}
  const pending = new Promise<void>((resolve) => {
    release = resolve
  })
  await act(async () => {
    root.render(
      <UsageLimitCard
        limit={limit}
        enabled={false}
        nowMs={reset - 1}
        onChange={() => pending}
      />,
    )
  })
  const box = host.querySelector("input")
  if (!(box instanceof HTMLInputElement)) throw new Error("missing checkbox")
  await act(async () => {
    box.click()
  })
  expect(box.checked).toBe(true)
  expect(
    host.querySelector("[data-usage-limit]")?.hasAttribute("data-usage-limit-pending"),
  ).toBe(true)
  expect(detail()).toBe(`Resets ${formatUsageLimitReset(reset)}`)
  await act(async () => {
    release()
  })
})

it("says scheduling failed once the confirmed preference is on and nothing is auto-resuming", async () => {
  await act(async () => {
    root.render(
      <UsageLimitCard limit={limit} enabled nowMs={reset - 1} onChange={() => {}} />,
    )
  })
  expect(detail()).toBe(usageLimitDetail(limit, true, reset - 1))
  expect(detail()).toBe("Scheduling failed.")
})
