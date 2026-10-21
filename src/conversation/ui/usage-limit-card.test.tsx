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

it("releases the checkbox when the confirmed preference is not the pending choice", async () => {
  let release: () => void = () => {}
  const held = new Promise<void>((resolve) => {
    release = resolve
  })
  let clicks = 0
  await act(async () => {
    root.render(
      <UsageLimitCard
        limit={limit}
        enabled={false}
        nowMs={reset - 1}
        onChange={() => {
          clicks += 1
          return held
        }}
      />,
    )
  })
  const box = host.querySelector("input")
  if (!(box instanceof HTMLInputElement)) throw new Error("missing checkbox")
  await act(async () => {
    box.click()
  })
  expect(box.checked).toBe(true)
  expect(clicks).toBe(1)
  await act(async () => {
    box.click()
  })
  expect(clicks).toBe(1)
  await act(async () => {
    release()
  })
  expect(box.checked).toBe(false)
  expect(
    host.querySelector("[data-usage-limit]")?.hasAttribute("data-usage-limit-pending"),
  ).toBe(false)
  expect(detail()).toBe(`Resets ${formatUsageLimitReset(reset)}`)
  let again: () => void = () => {}
  const next = new Promise<void>((resolve) => {
    again = resolve
  })
  await act(async () => {
    root.render(
      <UsageLimitCard
        limit={limit}
        enabled={false}
        nowMs={reset - 1}
        onChange={() => {
          clicks += 1
          return next
        }}
      />,
    )
  })
  const retry = host.querySelector("input")
  if (!(retry instanceof HTMLInputElement)) throw new Error("missing checkbox")
  await act(async () => {
    retry.click()
  })
  expect(clicks).toBe(2)
  expect(retry.checked).toBe(true)
  expect(
    host.querySelector("[data-usage-limit]")?.hasAttribute("data-usage-limit-pending"),
  ).toBe(true)
  await act(async () => {
    again()
  })
})

it("a superseded toggle completion does not settle the next choice", async () => {
  let settleFirst: () => void = () => {}
  const first = new Promise<void>((resolve) => {
    settleFirst = resolve
  })
  let settleSecond: () => void = () => {}
  const second = new Promise<void>((resolve) => {
    settleSecond = resolve
  })
  const calls: boolean[] = []
  function change(next: boolean) {
    calls.push(next)
    return calls.length === 1 ? first : second
  }
  await act(async () => {
    root.render(
      <UsageLimitCard
        limit={limit}
        enabled={false}
        nowMs={reset - 1}
        onChange={change}
      />,
    )
  })
  const box = () => {
    const node = host.querySelector("input")
    if (!(node instanceof HTMLInputElement)) throw new Error("missing checkbox")
    return node
  }
  const pending = () =>
    host.querySelector("[data-usage-limit]")?.hasAttribute("data-usage-limit-pending") ===
    true
  await act(async () => {
    box().click()
  })
  expect(calls).toEqual([true])
  expect(pending()).toBe(true)
  await act(async () => {
    root.render(
      <UsageLimitCard limit={limit} enabled nowMs={reset - 1} onChange={change} />,
    )
  })
  expect(pending()).toBe(false)
  await act(async () => {
    box().click()
  })
  expect(calls).toEqual([true, false])
  expect(box().checked).toBe(false)
  expect(pending()).toBe(true)
  await act(async () => {
    settleFirst()
  })
  expect(box().checked).toBe(false)
  expect(pending()).toBe(true)
  await act(async () => {
    box().click()
  })
  expect(calls).toEqual([true, false])
  await act(async () => {
    settleSecond()
  })
})

it("a superseded toggle rejection does not clear the next choice", async () => {
  let rejectFirst: (reason?: unknown) => void = () => {}
  const first = new Promise<void>((_, reject) => {
    rejectFirst = reject
  })
  let settleSecond: () => void = () => {}
  const second = new Promise<void>((resolve) => {
    settleSecond = resolve
  })
  const calls: boolean[] = []
  function change(next: boolean) {
    calls.push(next)
    return calls.length === 1 ? first : second
  }
  await act(async () => {
    root.render(
      <UsageLimitCard
        limit={limit}
        enabled={false}
        nowMs={reset - 1}
        onChange={change}
      />,
    )
  })
  const box = () => {
    const node = host.querySelector("input")
    if (!(node instanceof HTMLInputElement)) throw new Error("missing checkbox")
    return node
  }
  const pending = () =>
    host.querySelector("[data-usage-limit]")?.hasAttribute("data-usage-limit-pending") ===
    true
  await act(async () => {
    box().click()
  })
  await act(async () => {
    root.render(
      <UsageLimitCard limit={limit} enabled nowMs={reset - 1} onChange={change} />,
    )
  })
  await act(async () => {
    box().click()
  })
  expect(calls).toEqual([true, false])
  expect(pending()).toBe(true)
  await act(async () => {
    rejectFirst(new Error("the first toggle failed"))
  })
  expect(box().checked).toBe(false)
  expect(pending()).toBe(true)
  await act(async () => {
    box().click()
  })
  expect(calls).toEqual([true, false])
  await act(async () => {
    settleSecond()
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
