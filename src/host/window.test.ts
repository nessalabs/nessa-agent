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
  /** What the host reports when every step of the handoff did what it should. */
  const landed = { setupClosed: true, closeError: null, recordError: null }

  it("hands the host the one fact it does not have, and carries on", async () => {
    const { finishSetupWindow } = await import("./window")
    invoke.mockResolvedValue(landed)
    await expect(finishSetupWindow(true, "codex")).resolves.toEqual({
      outcome: "handed-over",
    })
    // One call, not three. Showing the panel, writing setup off and closing this
    // window are ordered on the host, in the process that outlives this window.
    expect(invoke).toHaveBeenCalledTimes(1)
    expect(invoke).toHaveBeenCalledWith("finish_setup", {
      completed: true,
      agent: "codex",
    })
    // Nothing closes the window from here any more.
    expect(close).not.toHaveBeenCalled()
  })

  it("tells the host that somebody left rather than finished", async () => {
    const { finishSetupWindow } = await import("./window")
    invoke.mockResolvedValue(landed)
    await expect(finishSetupWindow(false)).resolves.toEqual({ outcome: "handed-over" })
    // Leaving stays free to change its mind: the host records nothing for it,
    // and there is no choice to carry.
    expect(invoke).toHaveBeenCalledWith("finish_setup", {
      completed: false,
      agent: null,
    })
  })

  it("reads back the agent setup chose, and settles for the default without one", async () => {
    const { loadChosenAgent } = await import("./window")
    invoke.mockResolvedValue("codex")
    await expect(loadChosenAgent()).resolves.toBe("codex")
    expect(invoke).toHaveBeenCalledWith("chosen_agent")
    invoke.mockResolvedValue(null)
    await expect(loadChosenAgent()).resolves.toBeUndefined()
  })

  it("does not take the panel down over an unreadable choice", async () => {
    // The conversation starts on the gateway's own default instead, which is a
    // worse answer and not a broken panel.
    const { loadChosenAgent } = await import("./window")
    invoke.mockRejectedValue(new Error("no settings file"))
    await expect(loadChosenAgent()).resolves.toBeUndefined()
  })

  it("says the panel did not come up rather than closing over nothing", async () => {
    const { finishSetupWindow } = await import("./window")
    const cause = new Error("there is no panel to summon")
    invoke.mockRejectedValue(cause)
    await expect(finishSetupWindow(true)).resolves.toEqual({
      outcome: "panel-unavailable",
      cause,
    })
    expect(close).not.toHaveBeenCalled()
  })

  // The defect this distinction exists for: the panel was up, and the surface
  // said "Nessa could not open the panel" over the top of it.
  it("reports a window that would not close as exactly that, panel and all", async () => {
    const { finishSetupWindow } = await import("./window")
    invoke.mockResolvedValue({
      setupClosed: false,
      closeError: "could not close setup: the window server said no",
      recordError: null,
    })
    await expect(finishSetupWindow(true)).resolves.toEqual({
      outcome: "setup-close-failed",
      panelShown: true,
      cause: "could not close setup: the window server said no",
    })
  })

  // Survivable: a settings file that would not take the flag costs a second run
  // of setup, not the panel somebody is waiting on.
  it("hands over anyway when the completion write was refused", async () => {
    const { finishSetupWindow } = await import("./window")
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    invoke.mockResolvedValue({
      setupClosed: true,
      closeError: null,
      recordError: "could not record that setup finished: disk full",
    })
    await expect(finishSetupWindow(true)).resolves.toEqual({ outcome: "handed-over" })
    expect(warn).toHaveBeenCalledWith(
      "[nessa] could not record that setup finished",
      "could not record that setup finished: disk full",
    )
    warn.mockRestore()
  })

  it("has no second window to hand over to outside the desktop host", async () => {
    vi.stubGlobal("window", {})
    vi.resetModules()
    const { finishSetupWindow } = await import("./window")
    await expect(finishSetupWindow(true)).resolves.toEqual({ outcome: "no-native-host" })
    expect(invoke).not.toHaveBeenCalled()
  })
})
