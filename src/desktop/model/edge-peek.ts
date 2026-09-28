/**
 * Dock-style reveal of a collapsed sidebar, as a pure state machine. The
 * adapter owns the clock: it starts a timer for `pending` and sends the
 * matching `*-due` event when it fires. ADR 238 › _Side columns_ links here;
 * this table is the one owner of the peek's rules, and each row has a test in
 * `edge-peek.test.ts`.
 *
 * | state              | event                  | next                                              |
 * | ------------------ | ---------------------- | ------------------------------------------------- |
 * | hidden             | enter                  | hidden, reveal pending                            |
 * | hidden, reveal     | leave                  | hidden                                            |
 * | hidden, reveal     | reveal-due             | shown                                             |
 * | shown              | enter                  | shown (any hide cancelled)                        |
 * | shown              | leave                  | shown, hide pending                               |
 * | shown, hide        | hide-due               | hidden                                            |
 * | not pressed        | press                  | pressed; a reveal or hide pending is cancelled    |
 * | pressed            | enter, leave           | unchanged but for where the pointer is (`inside`) |
 * | pressed            | release                | not pressed; then as an enter if inside, else as a leave |
 * | not pressed        | release                | unchanged                                         |
 * | shown              | dock                   | shown, handoff pending                            |
 * | shown, handoff     | enter, leave           | unchanged but for `inside`                        |
 * | shown, handoff     | press                  | pressed; the handoff goes on                      |
 * | shown, handoff     | handoff-due            | hidden, handed off                                |
 * | hidden             | dock                   | hidden                                            |
 * | any                | dismiss                | hidden                                            |
 *
 * `enter` and `leave` cover both the edge strip and the revealed sidebar:
 * the pointer moving from one to the other is a leave then an enter, and the
 * enter cancels the hide the leave started. A stale `*-due` (its timer was
 * superseded, or a press cancelled it) changes nothing.
 *
 * **A press freezes the peek.** While a pointer button is held — a pane or a
 * session carried, a text selection — nothing but a dock or a dismiss
 * changes whether it is shown: the reveal or hide on its way is cancelled as
 * the button goes down, so a drag never finds it sliding away mid-carry, and
 * entering and leaving only record where the pointer is. At the release,
 * that decides. The adapter sends `release` for a release it missed, too —
 * at the next enter or leave with no button held, or the window's blur — so
 * the peek is never left frozen.
 *
 * `dock` is the sidebar being opened for real while revealed. The revealed
 * copy stays in place while the docked one opens beneath it, then goes
 * without animating (`handedOff`), so docking looks like the sidebar simply
 * staying.
 */
export interface EdgePeek {
  readonly shown: boolean
  readonly pending: "reveal" | "hide" | "handoff" | null
  readonly handedOff: boolean
  /** A pointer button is held: the peek stays as the press found it. */
  readonly pressed: boolean
  /** The pointer or keyboard focus is in the edge strip or the revealed sidebar. */
  readonly inside: boolean
}

export type EdgePeekEvent =
  | "enter"
  | "leave"
  | "press"
  | "release"
  | "reveal-due"
  | "hide-due"
  | "dock"
  | "handoff-due"
  | "dismiss"

export const edgePeekHidden: EdgePeek = {
  shown: false,
  pending: null,
  handedOff: false,
  pressed: false,
  inside: false,
}

/** Where the pointer now is, decided as the table says for a peek no button holds. */
function arrive(state: EdgePeek, inside: boolean): EdgePeek {
  if (inside)
    return state.shown
      ? { ...state, inside, pending: null, handedOff: false }
      : { ...state, inside, pending: "reveal", handedOff: false }
  return state.shown
    ? { ...state, inside, pending: "hide" }
    : { ...state, inside, pending: null, handedOff: false }
}

export function stepEdgePeek(state: EdgePeek, event: EdgePeekEvent): EdgePeek {
  switch (event) {
    case "enter":
    case "leave": {
      const inside = event === "enter"
      if (state.pressed || state.pending === "handoff")
        return state.inside === inside ? state : { ...state, inside }
      return arrive(state, inside)
    }
    case "press":
      if (state.pressed) return state
      return {
        ...state,
        pressed: true,
        pending: state.pending === "handoff" ? "handoff" : null,
      }
    case "release":
      if (!state.pressed) return state
      if (state.pending === "handoff") return { ...state, pressed: false }
      return arrive({ ...state, pressed: false }, state.inside)
    case "reveal-due":
      return state.pending === "reveal" ? { ...state, shown: true, pending: null } : state
    case "hide-due":
      return state.pending === "hide"
        ? { ...state, shown: false, pending: null, handedOff: false }
        : state
    case "dock":
      return state.shown
        ? { ...state, pending: "handoff" }
        : { ...state, shown: false, pending: null, handedOff: false }
    case "handoff-due":
      return state.pending === "handoff"
        ? { ...state, shown: false, pending: null, handedOff: true }
        : state
    case "dismiss":
      return { ...state, shown: false, pending: null, handedOff: false }
  }
}
