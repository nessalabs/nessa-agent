/**
 * The host callbacks a widget's view is given (`WidgetHost`, ADR 326), in each
 * place the workspace draws one, carried out by the workspace's own
 * commands. Each keeps its identity while what it is for does, so a view may
 * depend on it.
 *
 * ```text
 *   place    open(place)                        close()          openWidget(ref, place)       onEscape
 *   inline   a pane beside its conversation,    —                beside its conversation,      — (a card
 *            or the window                                        or the window                 has none)
 *   pane     the window; a pane: where it is    its pane closes  beside the pane showing       its pane's
 *                                                                 the widget's origin           steps back
 *   window   a pane beside its origin; the      back to the      beside its origin's pane,     the window's
 *            window: where it is                panes            or the window, replacing it    steps back
 * ```
 */
import { useMemo } from "react"
import type { PaneKey } from "../../../split-panes/model/pane-layout"
import type { EscapeStack, OpenPlace, WidgetHost, WidgetRef } from "../../../widgets"
import { closePane, openWidget, showContent } from "./commands"
import { useWorkspaceDispatch } from "./hooks"

const noStepBack = () => () => {}

/** For a card in a message of session `sessionId`'s: its pane goes beside that conversation. */
export function useInlineWidgetHost(widget: WidgetRef, sessionId: string): WidgetHost {
  const dispatch = useWorkspaceDispatch()
  return useMemo(
    () => ({
      open: (place: OpenPlace) =>
        dispatch(openWidget({ widget, place, origin: sessionId })),
      close: () => {},
      openWidget: (other: WidgetRef, place: OpenPlace) =>
        dispatch(openWidget({ widget: other, place, origin: sessionId })),
      onEscape: noStepBack,
    }),
    [dispatch, widget, sessionId],
  )
}

/** For a widget in pane `pane`, belonging to session `origin` if any. */
export function usePaneWidgetHost(
  pane: PaneKey,
  widget: WidgetRef,
  origin: string | undefined,
  steps: EscapeStack,
): WidgetHost {
  const dispatch = useWorkspaceDispatch()
  return useMemo(
    () => ({
      open: (place: OpenPlace) => {
        if (place === "window") dispatch(openWidget({ widget, place }))
      },
      close: () => dispatch(closePane({ pane })),
      openWidget: (other: WidgetRef, place: OpenPlace) =>
        dispatch(openWidget({ widget: other, place, origin })),
      onEscape: steps.push,
    }),
    [dispatch, pane, widget, origin, steps],
  )
}

/** For the widget the window shows, belonging to session `origin` if any. */
export function useWindowWidgetHost(
  widget: WidgetRef,
  origin: string | undefined,
  steps: EscapeStack,
): WidgetHost {
  const dispatch = useWorkspaceDispatch()
  return useMemo(
    () => ({
      open: (place: OpenPlace) => {
        if (place === "pane") dispatch(openWidget({ widget, place, origin }))
      },
      close: () => dispatch(showContent({ content: "panes" })),
      openWidget: (other: WidgetRef, place: OpenPlace) =>
        dispatch(openWidget({ widget: other, place, origin })),
      onEscape: steps.push,
    }),
    [dispatch, widget, origin, steps],
  )
}

/** What a session's header gives each plugin's accessory: widgets opened beside that session. */
export function useAccessoryOpen(
  sessionId: string,
): (widget: WidgetRef, place: OpenPlace) => void {
  const dispatch = useWorkspaceDispatch()
  return useMemo(
    () => (widget: WidgetRef, place: OpenPlace) =>
      dispatch(openWidget({ widget, place, origin: sessionId })),
    [dispatch, sessionId],
  )
}
