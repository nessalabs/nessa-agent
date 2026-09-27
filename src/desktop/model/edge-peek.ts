/**
 * Dock-style reveal of a collapsed sidebar, as a pure state machine. The
 * adapter owns the clock: it starts a timer for `pending` and sends the
 * matching `*-due` event when it fires.
 *
 * | state            | event        | next                      |
 * | ---------------- | ------------ | ------------------------- |
 * | hidden           | enter        | hidden, reveal pending    |
 * | hidden, reveal   | leave        | hidden                    |
 * | hidden, reveal   | reveal-due   | shown                     |
 * | shown            | enter        | shown (any hide cancelled)|
 * | shown            | leave        | shown, hide pending       |
 * | shown, hide      | hide-due     | hidden                    |
 * | any              | dismiss      | hidden                    |
 *
 * `enter` and `leave` cover both the edge strip and the revealed sidebar:
 * the pointer moving from one to the other is a leave then an enter, and the
 * enter cancels the hide the leave started. A stale `*-due` (its timer was
 * superseded) changes nothing.
 */
export interface EdgePeek {
  shown: boolean
  pending: "reveal" | "hide" | null
}

export type EdgePeekEvent = "enter" | "leave" | "reveal-due" | "hide-due" | "dismiss"

export const edgePeekHidden: EdgePeek = { shown: false, pending: null }

export function stepEdgePeek(state: EdgePeek, event: EdgePeekEvent): EdgePeek {
  switch (event) {
    case "enter":
      return state.shown
        ? { shown: true, pending: null }
        : { shown: false, pending: "reveal" }
    case "leave":
      return state.shown ? { shown: true, pending: "hide" } : edgePeekHidden
    case "reveal-due":
      return state.pending === "reveal" ? { shown: true, pending: null } : state
    case "hide-due":
      return state.pending === "hide" ? edgePeekHidden : state
    case "dismiss":
      return edgePeekHidden
  }
}
