import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { superviseSession } from "../session/adapters/lifecycle/supervisor"
import { createSessionHandle } from "../session/adapters/client/handle"
import type { EstablishedDevSession } from "../session/adapters/client/dev-session"

const { invoke, close } = vi.hoisted(() => ({ invoke: vi.fn(), close: vi.fn() }))
vi.mock("@tauri-apps/api/core", () => ({ invoke }))
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ close }) }))

beforeEach(() => {
  vi.resetModules()
  invoke.mockReset()
  close.mockReset()
  close.mockResolvedValue(undefined)
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

describe("closing the setup window", () => {
  it("reports the close it performed", async () => {
    const { closeSetupWindow } = await import("./window")
    await expect(closeSetupWindow()).resolves.toEqual({ outcome: "closed" })
    expect(close).toHaveBeenCalledTimes(1)
  })

  // The setup surface offers this from a screen whose only other control is a
  // retry. A rejection out of that handler would take both away with it.
  it("returns a refused close rather than throwing it", async () => {
    const { closeSetupWindow } = await import("./window")
    const cause = new Error("the window server said no")
    close.mockRejectedValue(cause)
    await expect(closeSetupWindow()).resolves.toEqual({
      outcome: "close-failed",
      cause,
    })
  })

  it("has no window of its own to close outside the desktop host", async () => {
    vi.stubGlobal("window", {})
    vi.resetModules()
    const { closeSetupWindow } = await import("./window")
    await expect(closeSetupWindow()).resolves.toEqual({ outcome: "no-native-host" })
    expect(close).not.toHaveBeenCalled()
  })
})

describe("handing setup over to the panel", () => {
  it("summons the panel and closes this window", async () => {
    const { finishSetupWindow } = await import("./window")
    invoke.mockResolvedValue(undefined)
    await expect(finishSetupWindow()).resolves.toEqual({ outcome: "handed-over" })
    expect(invoke).toHaveBeenCalledWith("summon_panel")
    expect(close).toHaveBeenCalledTimes(1)
  })

  it("says the panel did not come up rather than closing over nothing", async () => {
    const { finishSetupWindow } = await import("./window")
    const cause = new Error("no panel")
    invoke.mockRejectedValue(cause)
    await expect(finishSetupWindow()).resolves.toEqual({
      outcome: "panel-unavailable",
      cause,
    })
    expect(close).not.toHaveBeenCalled()
  })
})
