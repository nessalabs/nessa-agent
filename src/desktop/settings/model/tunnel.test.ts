/**
 * The web tunnel's reducer: one request at a time, nothing optimistic, a
 * restart held until the gateway that comes back answers, and a token that
 * is sent once and never read back.
 */
import { describe, expect, it } from "vitest"
import type { Outcome } from "./linked-devices"
import {
  canApply,
  canPoll,
  canToggle,
  initialTunnelState,
  tunnelDetail,
  tunnelFootnote,
  tunnelReducer,
  tunnelShown,
  type TunnelEvent,
  type TunnelRead,
  type TunnelSaved,
  type TunnelState,
} from "./tunnel"

const off: TunnelRead = {
  enabled: false,
  mode: "quick",
  hasToken: false,
  ingress: "127.0.0.1:47651",
  status: "off",
}

const ready: TunnelRead = {
  ...off,
  enabled: true,
  status: "ready",
  address: "https://a-b-c.trycloudflare.com",
}

function run(state: TunnelState, ...events: TunnelEvent[]): TunnelState {
  return events.reduce(tunnelReducer, state)
}

/** Answer the request in flight. */
function answer(state: TunnelState, outcome: Outcome<TunnelRead> | Outcome<TunnelSaved>) {
  if (!state.pending) throw new Error("nothing in flight")
  return tunnelReducer(state, { type: "answered", seq: state.pending.seq, outcome })
}

function connectedWith(read: TunnelRead): TunnelState {
  const asked = run(initialTunnelState(), { type: "connected" })
  expect(asked.pending?.kind).toBe("read")
  return answer(asked, { ok: true, value: read })
}

const saved = (value: TunnelSaved): Outcome<TunnelSaved> => ({ ok: true, value })

describe("web tunnel", () => {
  it("reads once connected, and says where it stands", () => {
    expect(tunnelDetail(initialTunnelState())).toContain("Checking")
    const state = connectedWith(off)
    expect(state.read).toEqual(off)
    expect(tunnelDetail(state)).toContain("same network")
    expect(canToggle(state)).toBe(true)
    expect(tunnelFootnote(state)).toContain("changes whenever cloudflared")
  })

  it("turning a quick tunnel on sends one change and holds the switch through the restart", () => {
    let state = run(connectedWith(off), { type: "toggle", enabled: true })
    expect(state.pending).toMatchObject({
      kind: "set",
      change: { enabled: true, mode: "quick" },
    })
    // A second flip while one is in flight sends nothing more.
    expect(run(state, { type: "toggle", enabled: false })).toBe(state)
    state = answer(state, saved({ enabled: true, mode: "quick", applies: "restarting" }))
    expect(tunnelShown(state)).toBe(true)
    expect(canToggle(state)).toBe(false)
    expect(state.notice).toBe("restarting")
    // A read before the drop still answers for the old gateway; none is sent.
    expect(state.pending).toBeNull()
    state = run(state, { type: "unreachable" }, { type: "connected" })
    expect(state.pending?.kind).toBe("read")
    state = answer(state, { ok: true, value: ready })
    expect(state.switching).toBeNull()
    expect(state.notice).toBeNull()
    expect(tunnelDetail(state)).toContain("from anywhere")
    expect(canPoll(state)).toBe(true)
  })

  it("a named tunnel needs a hostname and a token before it can be turned on", () => {
    let state = run(connectedWith(off), { type: "mode", mode: "named" })
    expect(canToggle(state)).toBe(false)
    state = run(state, { type: "hostname", hostname: " https://nessa.example.com " })
    expect(canToggle(state)).toBe(false)
    state = run(
      state,
      { type: "token", token: "eyJh" },
      { type: "toggle", enabled: true },
    )
    expect(state.pending).toMatchObject({
      kind: "set",
      change: {
        enabled: true,
        mode: "named",
        hostname: "https://nessa.example.com",
        token: "eyJh",
      },
    })
    state = answer(state, saved({ enabled: true, mode: "named", applies: "restarting" }))
    // Sent once: the field is empty again, and no read ever fills it.
    expect(state.draft.token).toBe("")
    expect(tunnelFootnote(state)).toContain("Restarting")
  })

  it("a saved token lets a named tunnel turn on with only a hostname", () => {
    const state = run(
      connectedWith({
        ...off,
        mode: "named",
        hostname: "https://n.example",
        hasToken: true,
      }),
      { type: "toggle", enabled: true },
    )
    expect(state.pending).toMatchObject({
      change: { enabled: true, mode: "named", hostname: "https://n.example" },
    })
    expect(state.pending?.kind === "set" && "token" in state.pending.change).toBe(false)
  })

  it("Apply is offered only for a tunnel that is on, with a changed, complete draft", () => {
    let state = connectedWith(ready)
    expect(canApply(state)).toBe(false)
    state = run(state, { type: "mode", mode: "named" })
    expect(canApply(state)).toBe(false)
    state = run(
      state,
      { type: "hostname", hostname: "https://n.example" },
      { type: "token", token: "t" },
    )
    expect(canApply(state)).toBe(true)
    state = run(state, { type: "apply" })
    expect(state.pending).toMatchObject({
      kind: "set",
      change: { enabled: true, mode: "named", hostname: "https://n.example", token: "t" },
    })
    expect(canApply(connectedWith(off))).toBe(false)
  })

  it("a draft being typed survives a poll; one with nothing unsent follows the gateway", () => {
    let state = run(connectedWith(ready), { type: "mode", mode: "named" })
    state = run(state, { type: "poll" })
    state = answer(state, { ok: true, value: { ...ready, address: "https://x.example" } })
    expect(state.draft.mode).toBe("named")
    state = run(state, { type: "mode", mode: "quick" }, { type: "poll" })
    state = answer(state, {
      ok: true,
      value: { ...ready, mode: "named", hostname: "https://n" },
    })
    expect(state.draft).toEqual({ mode: "named", hostname: "https://n", token: "" })
  })

  it("a refusal reads again and says why, never a wire code", () => {
    let state = run(connectedWith(off), { type: "toggle", enabled: true })
    state = answer(state, { ok: false, failure: { kind: "linkingOff" } })
    expect(state.notice).toBe("linkingOff")
    expect(state.pending?.kind).toBe("read")
    expect(tunnelFootnote(state)).toContain("Turn on native linking first")
    state = answer(state, { ok: true, value: off })
    expect(tunnelShown(state)).toBe(false)
    const busy = answer(run(connectedWith(off), { type: "toggle", enabled: true }), {
      ok: false,
      failure: { kind: "busy" },
    })
    expect(busy.notice).toBe("tunnelBusy")
    const taken = answer(run(connectedWith(off), { type: "toggle", enabled: true }), {
      ok: false,
      failure: { kind: "addressInUse" },
    })
    expect(tunnelFootnote(taken)).toContain("tunnel could not start")
  })

  it("a change whose answer was lost holds the switch until a read after the drop", () => {
    let state = run(connectedWith(off), { type: "toggle", enabled: true })
    state = answer(state, { ok: false, failure: { kind: "unanswered" } })
    expect(state.switching).toEqual({ enabled: true, dropped: false, confirmed: false })
    state = run(state, { type: "switchStalled" })
    expect(state.switching).toBeNull()
    expect(state.notice).toBe("unconfirmed")
    expect(state.pending?.kind).toBe("read")
    state = answer(state, { ok: true, value: off })
    expect(state.notice).toBeNull()
  })

  it("a confirmed restart that never comes back says the change is saved", () => {
    let state = run(connectedWith(off), { type: "toggle", enabled: true })
    state = answer(state, saved({ enabled: true, mode: "quick", applies: "restarting" }))
    state = run(state, { type: "unreachable" }, { type: "switchStalled" })
    expect(state.notice).toBe("stalled")
    expect(tunnelShown(state)).toBe(false)
    state = run(state, { type: "connected" })
    expect(state.notice).toBeNull()
    expect(state.pending?.kind).toBe("read")
  })

  it("a change that applies without a restart reads again", () => {
    let state = run(connectedWith(ready), { type: "toggle", enabled: false })
    expect(state.pending).toMatchObject({ change: { enabled: false } })
    state = answer(state, saved({ enabled: false, mode: "quick", applies: "unchanged" }))
    expect(state.switching).toBeNull()
    expect(state.pending?.kind).toBe("read")
  })

  it("a failed tunnel names why, and that it is tried again", () => {
    const state = connectedWith({
      ...ready,
      status: "failed",
      address: undefined,
      failure: "notReady",
    })
    expect(tunnelDetail(state)).toBe(
      "cloudflared started, but Cloudflare did not connect it. It is tried again.",
    )
  })

  it("an answer for a request since replaced changes nothing", () => {
    const state = run(initialTunnelState(), { type: "connected" })
    expect(
      tunnelReducer(state, {
        type: "answered",
        seq: 99,
        outcome: { ok: true, value: ready },
      }),
    ).toBe(state)
  })
})
