/** The lifecycle rows of the design table on #349, one at least one test. */
import { describe, expect, it } from "vitest"
import { advance, firstState, type Lifecycle, type LifecycleEvent } from "./lifecycle"
import type { Initialize } from "./messages"

const initialize: Initialize = { protocolVersion: "2026-01-26", appName: "App" }
const states: Record<Lifecycle["kind"], Lifecycle> = {
  reading: { kind: "reading" },
  proxy: { kind: "proxy" },
  loading: { kind: "loading" },
  initializing: { kind: "initializing", initialize },
  live: { kind: "live", initialize },
  ending: { kind: "ending", initialize },
  failed: { kind: "failed", reason: "load" },
  gone: { kind: "gone" },
}
const events: LifecycleEvent[] = [
  { kind: "read", outcome: "html" },
  { kind: "read", outcome: "unloadable" },
  { kind: "read", outcome: "server-gone" },
  { kind: "proxy-ready" },
  { kind: "app-left" },
  { kind: "initialize", initialize },
  { kind: "initialized" },
  { kind: "deadline" },
  { kind: "request-teardown", place: "pane" },
  { kind: "request-teardown", place: "inline" },
  { kind: "teardown-answered" },
  { kind: "removed" },
]

describe("the way through", () => {
  it("L1, L4, L6, L8: reading, the proxy, loading, initializing, live", () => {
    expect(firstState).toEqual({ kind: "reading" })
    let step = advance(firstState, { kind: "read", outcome: "html" })
    expect(step).toEqual({
      state: { kind: "proxy" },
      effects: [{ kind: "create-proxy" }, { kind: "deadline", for: "proxy" }],
    })
    step = advance(step.state, { kind: "proxy-ready" })
    expect(step).toEqual({
      state: { kind: "loading" },
      effects: [{ kind: "send-document" }, { kind: "deadline", for: "initialize" }],
    })
    step = advance(step.state, { kind: "initialize", initialize })
    expect(step).toEqual({
      state: { kind: "initializing", initialize },
      effects: [{ kind: "answer-initialize" }, { kind: "deadline", for: "initialize" }],
    })
    step = advance(step.state, { kind: "initialized" })
    expect(step).toEqual({
      state: { kind: "live", initialize },
      effects: [{ kind: "tell-call" }],
    })
  })

  it("L21, L23: a pane's or the window's app asks to go: teardown, then its place closes", () => {
    for (const place of ["pane", "window"] as const) {
      const ending = advance(states.live, { kind: "request-teardown", place })
      expect(ending).toEqual({
        state: { kind: "ending", initialize },
        effects: [{ kind: "send-teardown" }, { kind: "deadline", for: "teardown" }],
      })
      for (const event of [{ kind: "teardown-answered" }, { kind: "deadline" }] as const)
        expect(advance(ending.state, event)).toEqual({
          state: { kind: "gone" },
          effects: [{ kind: "close-place" }],
        })
    }
  })
})

describe("what fails", () => {
  it("L2, L3: a resource that cannot be loaded, or a server that is gone", () => {
    expect(advance(firstState, { kind: "read", outcome: "unloadable" }).state).toEqual({
      kind: "failed",
      reason: "load",
    })
    expect(advance(firstState, { kind: "read", outcome: "server-gone" }).state).toEqual({
      kind: "failed",
      reason: "server-gone",
    })
  })

  it("L5: a deadline before the app is live fails it to load, and takes its frame", () => {
    for (const kind of ["proxy", "loading", "initializing"] as const)
      expect(advance(states[kind], { kind: "deadline" }), kind).toEqual({
        state: { kind: "failed", reason: "load" },
        effects: [{ kind: "remove-frame" }],
      })
  })

  it("L11: the proxy ready again, under an app, fails it", () => {
    for (const kind of ["loading", "initializing", "live", "ending"] as const)
      expect(advance(states[kind], { kind: "proxy-ready" }), kind).toEqual({
        state: { kind: "failed", reason: "load" },
        effects: [{ kind: "remove-frame" }],
      })
  })
})

describe("the app leaving its frame", () => {
  it("L32: once the document is handed over, its frame loading again fails it and takes the frame", () => {
    for (const kind of ["loading", "initializing", "live", "ending"] as const)
      expect(advance(states[kind], { kind: "app-left" }), kind).toEqual({
        state: { kind: "failed", reason: "load" },
        effects: [{ kind: "remove-frame" }],
      })
    for (const kind of ["reading", "proxy"] as const)
      expect(advance(states[kind], { kind: "app-left" }).state, kind).toBe(states[kind])
  })
})

describe("what is ignored", () => {
  it("L22: an inline app asking to go is declined", () => {
    const step = advance(states.live, { kind: "request-teardown", place: "inline" })
    expect(step.state).toBe(states.live)
    expect(step.effects).toEqual([])
  })

  it("L24: removal ends every state but gone, with nothing to do", () => {
    for (const state of Object.values(states))
      expect(advance(state, { kind: "removed" }), state.kind).toEqual(
        state.kind === "gone"
          ? { state, effects: [] }
          : { state: { kind: "gone" }, effects: [] },
      )
  })

  it("gone and failed answer nothing, and an event out of its state changes nothing", () => {
    const moves = new Set([
      "reading read",
      "proxy proxy-ready",
      "loading initialize",
      "initializing initialized",
      "proxy deadline",
      "loading deadline",
      "initializing deadline",
      "ending deadline",
      "live request-teardown",
      "ending teardown-answered",
      "loading proxy-ready",
      "initializing proxy-ready",
      "live proxy-ready",
      "ending proxy-ready",
      "loading app-left",
      "initializing app-left",
      "live app-left",
      "ending app-left",
    ])
    for (const state of Object.values(states))
      for (const event of events) {
        if (event.kind === "removed") continue
        const moved =
          moves.has(`${state.kind} ${event.kind}`) &&
          !(event.kind === "request-teardown" && event.place === "inline")
        const step = advance(state, event)
        if (!moved) {
          expect(step.state, `${state.kind} ${event.kind}`).toBe(state)
          expect(step.effects, `${state.kind} ${event.kind}`).toEqual([])
        } else expect(step.state, `${state.kind} ${event.kind}`).not.toBe(state)
      }
  })
})
