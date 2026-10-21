// @vitest-environment jsdom

import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"

import { beginOnboarding, recordReadiness, startAgentChoice } from "../model/onboarding"
import { Onboarding } from "./onboarding"
import type { RuntimeCopy } from "../../host"

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia
  Element.prototype.animate ??= (() => ({
    finished: Promise.resolve(),
    cancel: () => {},
    finish: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof Element.prototype.animate
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

const state = recordReadiness(startAgentChoice(beginOnboarding()), { claude: "ready" })

function view(runtimeCopy?: RuntimeCopy) {
  return (
    <Onboarding
      state={state}
      gatewayStartup={{ revision: 1, state: "ready" }}
      platform="apple"
      runtimeCopy={runtimeCopy}
      onBegin={() => {}}
      onChoose={() => {}}
      onConfirm={() => {}}
      onFinish={() => {}}
      onRecheck={() => {}}
      onRetryGateway={() => {}}
    />
  )
}

function claude() {
  const button = [...container.querySelectorAll("button")].find(
    (item) => item.getAttribute("aria-label") === "Claude",
  )
  if (!button) throw new Error("Missing Claude")
  return button
}

function bar() {
  return container.querySelector("progress")
}

it("shows sync progress and leaves a ready agent selectable", async () => {
  const copying: RuntimeCopy = {
    state: "copying",
    filesCompleted: 0,
    filesTotal: 4,
    bytesCompleted: 0,
    bytesTotal: 40,
  }
  await React.act(async () => root.render(view(copying)))
  expect(bar()?.getAttribute("aria-label")).toBe("Preparing the private runtime")
  expect(bar()?.getAttribute("aria-valuenow")).toBe("0")
  expect(claude().getAttribute("aria-disabled")).toBeNull()

  await React.act(async () =>
    root.render(
      view({
        ...copying,
        filesCompleted: 2,
        bytesCompleted: 20,
      }),
    ),
  )
  expect(bar()?.getAttribute("aria-valuenow")).toBe("50")
  expect(claude().getAttribute("aria-disabled")).toBeNull()

  await React.act(async () =>
    root.render(
      view({
        state: "settled",
        filesCompleted: 0,
        filesTotal: 0,
        bytesCompleted: 0,
        bytesTotal: 0,
      }),
    ),
  )
  expect(bar()).toBeNull()
  expect(claude().getAttribute("aria-disabled")).toBeNull()
})

it("draws nothing when the host has not reported a copy", async () => {
  await React.act(async () => root.render(view()))
  expect(bar()).toBeNull()
  expect(claude().getAttribute("aria-disabled")).toBeNull()
})

it("keeps a nearly finished copy below 100 while a file is still syncing", async () => {
  await React.act(async () =>
    root.render(
      view({
        state: "copying",
        filesCompleted: 1,
        filesTotal: 2,
        bytesCompleted: 399,
        bytesTotal: 400,
      }),
    ),
  )
  expect(bar()?.getAttribute("aria-valuenow")).toBe("99")
})

it("keeps a byte-complete copy below 100 while a zero-length file remains", async () => {
  await React.act(async () =>
    root.render(
      view({
        state: "copying",
        filesCompleted: 1,
        filesTotal: 2,
        bytesCompleted: 400,
        bytesTotal: 400,
      }),
    ),
  )
  expect(bar()?.getAttribute("aria-valuenow")).toBe("99")
})

it("uses the file count when the copy has no byte total", async () => {
  await React.act(async () =>
    root.render(
      view({
        state: "copying",
        filesCompleted: 1,
        filesTotal: 4,
        bytesCompleted: 0,
        bytesTotal: 0,
      }),
    ),
  )
  expect(bar()?.getAttribute("aria-valuenow")).toBe("25")
})
