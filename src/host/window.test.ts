import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { superviseSession } from "../session/adapters/lifecycle/supervisor"
import { createSessionHandle } from "../session/adapters/client/handle"
import type { EstablishedDevSession } from "../session/adapters/client/dev-session"

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock("@tauri-apps/api/core", () => ({ invoke }))

beforeEach(() => {
  vi.resetModules()
  invoke.mockReset()
  vi.stubGlobal("window", { __TAURI_INTERNALS__: {} })
})
afterEach(() => vi.unstubAllGlobals())

describe("native surface credential failures", () => {
  it.each([
    "Move Nessa to Applications before starting its background service",
    { message: "The gateway update failed; existing agents were preserved" },
  ])("preserves safe native messages as Errors", async (failure) => {
    const { loadAssignedSurfaceCredential } = await import("./window")
    invoke.mockRejectedValue(failure)
    await expect(loadAssignedSurfaceCredential("prod")).rejects.toThrow(
      typeof failure === "string" ? failure : failure.message,
    )
    expect(invoke).toHaveBeenCalledWith("load_surface_credential", { stage: "prod" })
  })

  it.each([undefined, null, 42, "  ", { message: 4 }, { token: "secret" }])(
    "does not expose unrecognized native payloads",
    async (failure) => {
      const { loadAssignedSurfaceCredential } = await import("./window")
      invoke.mockRejectedValue(failure)
      await expect(loadAssignedSurfaceCredential("prod")).rejects.toThrow(
        "Could not load the desktop gateway credential.",
      )
    },
  )

  it("preserves an existing Error and its typed identity", async () => {
    const { loadAssignedSurfaceCredential } = await import("./window")
    const failure = new TypeError("native transport failed")
    invoke.mockRejectedValue(failure)
    await expect(loadAssignedSurfaceCredential("prod")).rejects.toBe(failure)
  })

  it("shows the native reason and explicit session retry invokes native setup again", async () => {
    const { loadAssignedSurfaceCredential } = await import("./window")
    invoke
      .mockRejectedValueOnce("Gateway update failed")
      .mockResolvedValueOnce("private-token")
    const client = {
      connectionState: { status: "connected" },
      productSession: {},
      close: vi.fn(),
      onClose: () => () => {},
      onConnectionStateChange: () => () => {},
    } as unknown as EstablishedDevSession["client"]
    const session = createSessionHandle()
    const failed = vi.fn()
    const ready = vi.fn()
    const connect = vi.fn(async () => {
      await loadAssignedSurfaceCredential("prod")
      return { client, hello: client.productSession, health: {} } as EstablishedDevSession
    })
    const options = {
      session,
      connect,
      connecting: vi.fn(),
      reconnecting: vi.fn(),
      failed,
      ready,
    }
    const stop = superviseSession(options)
    await vi.waitFor(() => expect(failed).toHaveBeenCalledWith("Gateway update failed"))
    expect(session.get()).toBeNull()
    expect(invoke).toHaveBeenCalledTimes(1)
    stop()
    const stopRetry = superviseSession(options)
    await vi.waitFor(() => expect(ready).toHaveBeenCalledOnce())
    expect(invoke).toHaveBeenCalledTimes(2)
    expect(session.get()).toBe(client)
    stopRetry()
  })
})
