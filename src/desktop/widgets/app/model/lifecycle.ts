/**
 * One app view's lifecycle: the states of the design table on #349, and the
 * events that move between them, as a pure function. The bridge
 * (`application/bridge.ts`) feeds it what happened and carries out what it
 * returns; requests in `live` are the bridge's, and do not move this.
 *
 * ```text
 *   reading ──html──▶ proxy ──proxy-ready──▶ loading ──ui/initialize──▶ initializing
 *      │                │                      │                            │
 *      │ unloadable,    │ deadline             │ deadline                   │ initialized
 *      │ server gone    ▼                      ▼                            ▼
 *      └────────────▶ failed ◀──── proxy-ready again, app-left (loading … ending) live
 *                                                                           │ request-teardown
 *                                                                           ▼ (pane, window)
 *   removed, from anywhere ──▶ gone ◀──── answered, or deadline ──────── ending
 * ```
 *
 * An arrow is an event; a state with no arrow for an event ignores it.
 */
import type { WidgetPlace } from "../../model/widget-state"
import type { Initialize } from "./messages"

/** Why a view could not be shown. */
export type FailedReason = "load" | "server-gone"

export type Lifecycle =
  /** The UI resource is being read. */
  | { readonly kind: "reading" }
  /** The sandbox proxy is loading. */
  | { readonly kind: "proxy" }
  /** The app's document was handed to the proxy; the app has not said `ui/initialize`. */
  | { readonly kind: "loading" }
  /** Answered `ui/initialize`; waiting for `ui/notifications/initialized`. */
  | { readonly kind: "initializing"; readonly initialize: Initialize }
  | { readonly kind: "live"; readonly initialize: Initialize }
  /** Asked to tear down (`ui/resource-teardown`), waiting for its answer. */
  | { readonly kind: "ending"; readonly initialize: Initialize }
  | { readonly kind: "failed"; readonly reason: FailedReason }
  /** Its place was removed, or it ended: nothing is sent to it again. */
  | { readonly kind: "gone" }

export type LifecycleEvent =
  | { readonly kind: "read"; readonly outcome: "html" | "unloadable" | "server-gone" }
  | { readonly kind: "proxy-ready" }
  /** The proxy found the app's document gone from its frame, and removed the frame (L32). */
  | { readonly kind: "app-left" }
  | { readonly kind: "initialize"; readonly initialize: Initialize }
  | { readonly kind: "initialized" }
  | { readonly kind: "deadline" }
  | { readonly kind: "request-teardown"; readonly place: WidgetPlace }
  | { readonly kind: "teardown-answered" }
  | { readonly kind: "removed" }

/** What the bridge does for a transition, in order. */
export type LifecycleEffect =
  | { readonly kind: "send-document" }
  | { readonly kind: "answer-initialize" }
  | { readonly kind: "tell-call" }
  | { readonly kind: "send-teardown" }
  | { readonly kind: "close-place" }
  /** A deadline for the state entered; every state change clears the one before. */
  | { readonly kind: "deadline"; readonly for: "proxy" | "initialize" | "teardown" }

export interface Step {
  readonly state: Lifecycle
  readonly effects: readonly LifecycleEffect[]
}

export const firstState: Lifecycle = { kind: "reading" }

/**
 * Whether the proxy's frame is on the page in `state`: from the resource
 * read to the app's end. The one statement of it — the view draws the frame
 * by it, and the bridge posts only while it holds.
 */
export function frameOn(state: Lifecycle): boolean {
  switch (state.kind) {
    case "proxy":
    case "loading":
    case "initializing":
    case "live":
    case "ending":
      return true
    case "reading":
    case "failed":
    case "gone":
      return false
  }
}

const stay = (state: Lifecycle): Step => ({ state, effects: [] })

/** The next state and its effects for `event` in `state`. */
export function advance(state: Lifecycle, event: LifecycleEvent): Step {
  if (state.kind === "gone") return stay(state)
  if (event.kind === "removed") return { state: { kind: "gone" }, effects: [] }
  if (state.kind === "failed") return stay(state)
  switch (event.kind) {
    case "read":
      if (state.kind !== "reading") return stay(state)
      if (event.outcome === "html")
        return {
          state: { kind: "proxy" },
          effects: [{ kind: "deadline", for: "proxy" }],
        }
      return {
        state: {
          kind: "failed",
          reason: event.outcome === "server-gone" ? "server-gone" : "load",
        },
        effects: [],
      }
    case "proxy-ready":
      if (state.kind === "proxy")
        return {
          state: { kind: "loading" },
          effects: [{ kind: "send-document" }, { kind: "deadline", for: "initialize" }],
        }
      // The proxy loaded again under a running app: what it shows now is
      // nothing the host handed it.
      if (state.kind === "reading") return stay(state)
      return {
        state: { kind: "failed", reason: "load" },
        effects: [],
      }
    case "app-left":
      // The app's document is no longer the one handed over: a navigation of
      // its frame, refused or not, or a reload.
      if (state.kind === "reading" || state.kind === "proxy") return stay(state)
      return {
        state: { kind: "failed", reason: "load" },
        effects: [],
      }
    case "initialize":
      if (state.kind !== "loading") return stay(state)
      return {
        state: { kind: "initializing", initialize: event.initialize },
        effects: [{ kind: "answer-initialize" }, { kind: "deadline", for: "initialize" }],
      }
    case "initialized":
      if (state.kind !== "initializing") return stay(state)
      return {
        state: { kind: "live", initialize: state.initialize },
        effects: [{ kind: "tell-call" }],
      }
    case "deadline":
      switch (state.kind) {
        case "proxy":
        case "loading":
        case "initializing":
          return {
            state: { kind: "failed", reason: "load" },
            effects: [],
          }
        case "ending":
          return { state: { kind: "gone" }, effects: [{ kind: "close-place" }] }
        default:
          return stay(state)
      }
    case "request-teardown":
      // A card in a message has no close: the request is declined.
      if (state.kind !== "live" || event.place === "inline") return stay(state)
      return {
        state: { kind: "ending", initialize: state.initialize },
        effects: [{ kind: "send-teardown" }, { kind: "deadline", for: "teardown" }],
      }
    case "teardown-answered":
      if (state.kind !== "ending") return stay(state)
      return { state: { kind: "gone" }, effects: [{ kind: "close-place" }] }
  }
}
