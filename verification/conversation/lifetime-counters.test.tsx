// @vitest-environment jsdom
/**
 * A cleanup after the last render is visible to the verifier, which reads
 * the fixture owner (`readLifetimeCounters`), not the attributes that render
 * copied.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { readLifetimeCounters } from "../desktop/scripts/lib/lifetime-snapshot.mjs"
import {
  createLifetimeCounters,
  publishLifetimeCounters,
  type LifetimeCounters,
} from "./lifetime-counters"

let host: HTMLDivElement
let root: Root

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
  delete window.__localTabLifetimes
})

/** The attributes a render would leave behind. */
function Stamp({ counters }: { counters: LifetimeCounters }) {
  const snap = counters.snapshot()
  return (
    <main
      data-follows={JSON.stringify(snap.follows)}
      data-stops={snap.stops}
      data-closed={JSON.stringify(snap.closed)}
    />
  )
}

it("reads counters updated after the last render", async () => {
  const counters = createLifetimeCounters()
  counters.noteFollow("a")
  await act(async () => root.render(<Stamp counters={counters} />))
  publishLifetimeCounters(counters)
  const rendered = {
    follows: host.querySelector("main")?.getAttribute("data-follows"),
    stops: host.querySelector("main")?.getAttribute("data-stops"),
    closed: host.querySelector("main")?.getAttribute("data-closed"),
  }
  counters.noteStop()
  counters.noteFollow("b")
  counters.noteClose("a")
  const live = await readLifetimeCounters(async (read: () => unknown) => read())
  expect(rendered).toEqual({ follows: '["a"]', stops: "0", closed: "[]" })
  expect(live).toEqual({ follows: ["a", "b"], stops: 1, closed: ["a"] })
  expect(host.querySelector("main")?.getAttribute("data-stops")).toBe("0")
})

it("fails when the page has no live owner", async () => {
  delete window.__localTabLifetimes
  await expect(
    readLifetimeCounters(async (read: () => unknown) => read()),
  ).rejects.toThrow("local tab lifetime counters have no live owner")
})
