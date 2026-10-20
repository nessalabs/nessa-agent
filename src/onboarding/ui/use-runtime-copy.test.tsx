// @vitest-environment jsdom

import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

import type { RuntimeCopy } from "../../host"

const host = vi.hoisted(() => ({
  onRuntimeCopy: vi.fn<(handler: (progress: RuntimeCopy) => void) => Promise<() => void>>(),
  runtimeCopy: vi.fn<() => Promise<RuntimeCopy | null>>(),
}))
vi.mock("../../host", () => host)

import { useRuntimeCopy } from "./use-runtime-copy"

const copying: RuntimeCopy = {
  state: "copying",
  filesCompleted: 1,
  filesTotal: 4,
  bytesCompleted: 10,
  bytesTotal: 40,
}

const settled: RuntimeCopy = {
  state: "settled",
  filesCompleted: 0,
  filesTotal: 0,
  bytesCompleted: 0,
  bytesTotal: 0,
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (cause: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

function Probe() {
  const progress = useRuntimeCopy()
  return <output>{progress ? `${progress.state}:${progress.filesCompleted}` : "absent"}</output>
}

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host.onRuntimeCopy.mockReset()
  host.runtimeCopy.mockReset()
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

async function render() {
  await React.act(async () => {
    root.render(<Probe />)
  })
}

function shown() {
  return container.querySelector("output")?.textContent
}

it("reads a report the host retained before the page subscribed", async () => {
  const listen = deferred<() => void>()
  host.onRuntimeCopy.mockReturnValue(listen.promise)
  host.runtimeCopy.mockResolvedValue(copying)

  await render()
  expect(shown()).toBe("absent")

  await React.act(async () => {
    listen.resolve(() => undefined)
  })

  expect(shown()).toBe("copying:1")
  expect(host.onRuntimeCopy.mock.invocationCallOrder[0]).toBeLessThan(
    host.runtimeCopy.mock.invocationCallOrder[0],
  )
})

it("keeps an event that arrives while the retained report is being read", async () => {
  let handler!: (progress: RuntimeCopy) => void
  const listen = deferred<() => void>()
  const snapshot = deferred<RuntimeCopy | null>()
  host.onRuntimeCopy.mockImplementation(async (next) => {
    handler = next
    return listen.promise
  })
  host.runtimeCopy.mockReturnValue(snapshot.promise)

  await render()
  await React.act(async () => {
    listen.resolve(() => undefined)
  })
  await React.act(async () => {
    handler(settled)
  })
  expect(shown()).toBe("settled:0")

  await React.act(async () => {
    snapshot.resolve(copying)
  })

  expect(shown()).toBe("settled:0")
})

it("stays absent when the host has not reported", async () => {
  host.onRuntimeCopy.mockResolvedValue(() => undefined)
  host.runtimeCopy.mockResolvedValue(null)

  await render()

  expect(shown()).toBe("absent")
})

it("keeps an event when the retained read fails", async () => {
  let handler!: (progress: RuntimeCopy) => void
  const listen = deferred<() => void>()
  const snapshot = deferred<RuntimeCopy | null>()
  host.onRuntimeCopy.mockImplementation(async (next) => {
    handler = next
    return listen.promise
  })
  host.runtimeCopy.mockReturnValue(snapshot.promise)

  await render()
  await React.act(async () => {
    listen.resolve(() => undefined)
  })
  await React.act(async () => {
    handler(copying)
  })
  await React.act(async () => {
    snapshot.reject(new Error("snapshot unavailable"))
  })

  expect(shown()).toBe("copying:1")
})

it("follows a later event after the retained report was applied", async () => {
  let handler!: (progress: RuntimeCopy) => void
  host.onRuntimeCopy.mockImplementation(async (next) => {
    handler = next
    return () => undefined
  })
  host.runtimeCopy.mockResolvedValue(copying)

  await render()
  expect(shown()).toBe("copying:1")

  await React.act(async () => {
    handler(settled)
  })

  expect(shown()).toBe("settled:0")
})
