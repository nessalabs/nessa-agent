/**
 * Dock-style reveal of a collapsed sidebar, as a pure state machine. The
 * adapter owns the clock: it starts a timer for `pending` and sends the
 * matching `*-due` event when it fires.
 *
 * | state            | event        | next                                  |
 * | ---------------- | ------------ | ------------------------------------- |
 * | hidden           | enter        | hidden, reveal pending                |
 * | hidden, reveal   | leave        | hidden                                |
 * | hidden, reveal   | reveal-due   | shown                                 |
 * | shown            | enter        | shown (any hide cancelled)            |
 * | shown            | leave        | shown, hide pending                   |
 * | shown, hide      | hide-due     | hidden                                |
 * | shown            | dock         | shown, handoff pending                |
 * | shown, handoff   | enter, leave | unchanged                             |
 * | shown, handoff   | handoff-due  | hidden, handed off                    |
 * | hidden           | dock         | hidden                                |
 * | any              | dismiss      | hidden                                |
 *
 * `enter` and `leave` cover both the edge strip and the revealed sidebar:
 * the pointer moving from one to the other is a leave then an enter, and the
 * enter cancels the hide the leave started. A stale `*-due` (its timer was
 * superseded) changes nothing.
 *
 * `dock` is the sidebar being opened for real while revealed. The revealed
 * copy stays in place while the docked one opens beneath it, then goes
 * without animating (`handedOff`), so docking looks like the sidebar simply
 * staying.
 */
export interface EdgePeek {
  shown: boolean
  pending: "reveal" | "hide" | "handoff" | null
  handedOff: boolean
}

export type EdgePeekEvent =
  "enter" | "leave" | "reveal-due" | "hide-due" | "dock" | "handoff-due" | "dismiss"

export const edgePeekHidden: EdgePeek = { shown: false, pending: null, handedOff: false }

export function stepEdgePeek(state: EdgePeek, event: EdgePeekEvent): EdgePeek {
  if (state.pending === "handoff" && (event === "enter" || event === "leave"))
    return state
  switch (event) {
    case "enter":
      return state.shown
        ? { shown: true, pending: null, handedOff: false }
        : { shown: false, pending: "reveal", handedOff: false }
    case "leave":
      return state.shown ? { ...state, pending: "hide" } : edgePeekHidden
    case "reveal-due":
      return state.pending === "reveal" ? { ...state, shown: true, pending: null } : state
    case "hide-due":
      return state.pending === "hide" ? edgePeekHidden : state
    case "dock":
      return state.shown ? { ...state, pending: "handoff" } : edgePeekHidden
    case "handoff-due":
      return state.pending === "handoff"
        ? { shown: false, pending: null, handedOff: true }
        : state
    case "dismiss":
      return edgePeekHidden
  }
}
