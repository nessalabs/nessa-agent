/**
 * The host callbacks a widget's view is given (`WidgetHost`, ADR 326), in each
 * place the workspace draws one, carried out by the workspace's own
 * commands. Each keeps its identity while what it is for does, so a view may
 * depend on it.
 *
 * ```text
 *   place    open(place)                        close()          openWidget(ref, place)       onEscape
 *   inline   a tab in its conversation's        —                a tab in its conversation's   — (a card
 *            panel, or the window                                 panel, or the window          has none)
 *   panel    the window; the panel: where it    its tab closes   a tab in the panel of the     the panel's
 *            is                                                   widget's origin, else this    steps back
 *                                                                 chat's
 *   window   a tab in its origin's panel; the   back to the      a tab in its origin's panel,  the window's
 *            window: where it is                panes            or the window, replacing it    steps back
 * ```
 */
import { useMemo } from "react"
import type {
  EscapeStack,
  OpenPlace,
  WidgetCrumb,
  WidgetHost,
  WidgetRef,
} from "../../../widgets"
import { closePanelTab, openSession, openWidget, showContent } from "./commands"
import { useWorkspaceDispatch } from "./hooks"

const noStepBack = () => () => {}

/** For a card in a message of session `sessionId`'s: it opens as a tab in that conversation's panel. */
export function useInlineWidgetHost(widget: WidgetRef, sessionId: string): WidgetHost {
  const dispatch = useWorkspaceDispatch()
  return useMemo(
    () => ({
      open: (place: OpenPlace) =>
        dispatch(openWidget({ widget, place, origin: sessionId })),
      close: () => {},
      openWidget: (other: WidgetRef, place: OpenPlace) =>
        dispatch(openWidget({ widget: other, place, origin: sessionId })),
      openSession: (other: string) => dispatch(openSession({ sessionId: other })),
      onEscape: noStepBack,
    }),
    [dispatch, widget, sessionId],
  )
}

/**
 * For a widget in chat `sessionId`'s panel, belonging to session `origin` if
 * any: what it opens goes to its origin's panel, else to this chat's.
 */
export function usePanelWidgetHost(
  sessionId: string,
  widget: WidgetRef,
  origin: string | undefined,
  steps: EscapeStack,
  crumb?: (step: WidgetCrumb | null) => void,
): WidgetHost {
  const dispatch = useWorkspaceDispatch()
  return useMemo(
    () => ({
      open: (place: OpenPlace) => {
        if (place === "window") dispatch(openWidget({ widget, place }))
      },
      close: () => dispatch(closePanelTab({ sessionId, widget })),
      openWidget: (other: WidgetRef, place: OpenPlace) =>
        dispatch(openWidget({ widget: other, place, origin: origin ?? sessionId })),
      openSession: (other: string) => dispatch(openSession({ sessionId: other })),
      onEscape: steps.push,
      crumb,
    }),
    [dispatch, sessionId, widget, origin, steps, crumb],
  )
}

/** For the widget the window shows, belonging to session `origin` if any. */
export function useWindowWidgetHost(
  widget: WidgetRef,
  origin: string | undefined,
  steps: EscapeStack,
  crumb?: (step: WidgetCrumb | null) => void,
): WidgetHost {
  const dispatch = useWorkspaceDispatch()
  return useMemo(
    () => ({
      open: (place: OpenPlace) => {
        if (place === "panel") dispatch(openWidget({ widget, place, origin }))
      },
      close: () => dispatch(showContent({ content: "panes" })),
      openWidget: (other: WidgetRef, place: OpenPlace) =>
        dispatch(openWidget({ widget: other, place, origin })),
      openSession: (other: string) => dispatch(openSession({ sessionId: other })),
      onEscape: steps.push,
      crumb,
    }),
    [dispatch, widget, origin, steps, crumb],
  )
}

/** What a session's header gives each plugin's accessory: widgets opened in that session's panel. */
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
