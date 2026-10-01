import { memo, useCallback, useRef, type RefObject } from "react"
import {
  offeredBy,
  offersWindow,
  WidgetAnswerOf,
  WidgetBody,
  widgetOrigin,
  widgetTitle,
  type EscapeStack,
  type WidgetAnswer,
  type WidgetPlugin,
  type WidgetRef,
} from "../../../widgets"
import { paneScope, useEscapeScope } from "../../adapters/dom/widget-escape"
import { focusedPaneAttribute } from "../../adapters/dom/focus"
import { closePane, openBeside } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectFocusedPaneKey, selectPaneClosable } from "../../adapters/store/selectors"
import { usePaneWidgetHost } from "../../adapters/store/widget-hosts"
import type { PaneFrame } from "../../../split-panes"
import type { PaneKey } from "../../../split-panes/model/pane-layout"
import { IconButton } from "../chrome/icon-button"
import { useWorkspaceFrame } from "../workspace-frame"
import { usePaneFocus } from "./use-pane-focus"
import { WidgetTrail } from "./widget-trail"

/**
 * A pane showing a widget (ADR 326): a session pane's glass, header rhythm
 * and place in the grid, so it splits, resizes, moves and closes as any
 * pane does. Its header names the widget after the way back to the
 * conversation it belongs to, and offers the window (expand) where the
 * plugin draws one, and close; the widget's host draws the rest
 * (`WidgetBody`), and its view's steps back answer Escape while focus is in
 * the pane.
 */
export const WidgetPane = memo(function WidgetPane({
  pane,
  widget,
  frame,
  multi,
}: {
  pane: PaneKey
  widget: WidgetRef
  /** What the grid puts on the pane's root: where it is placed, and its names (`SplitPanes`). */
  frame: PaneFrame
  multi: boolean
}) {
  const root = useRef<HTMLElement>(null)
  const steps = useEscapeScope(paneScope(pane), root)
  return (
    <WidgetAnswerOf widget={widget}>
      {(answer, plugin) => (
        <WidgetPaneParts
          root={root}
          steps={steps}
          pane={pane}
          widget={widget}
          frame={frame}
          multi={multi}
          answer={answer}
          plugin={plugin}
        />
      )}
    </WidgetAnswerOf>
  )
})

function WidgetPaneParts({
  root,
  steps,
  pane,
  widget,
  frame,
  multi,
  answer,
  plugin,
}: {
  root: RefObject<HTMLElement | null>
  steps: EscapeStack
  pane: PaneKey
  widget: WidgetRef
  frame: PaneFrame
  multi: boolean
  answer: WidgetAnswer
  plugin: WidgetPlugin | undefined
}) {
  const dispatch = useWorkspaceDispatch()
  const shortcuts = useWorkspaceFrame()
  const focusHandlers = usePaneFocus(pane)
  const focused = useWorkspaceSelector((state) => selectFocusedPaneKey(state) === pane)
  const closable = useWorkspaceSelector((state) => selectPaneClosable(state, pane))
  const origin = widgetOrigin(answer)
  const title = widgetTitle(answer)
  const host = usePaneWidgetHost(pane, widget, origin, steps)
  // Back to the conversation: focused where it is, or opened beside this pane.
  const toOrigin = useCallback(
    (sessionId: string) => dispatch(openBeside({ sessionId, target: pane })),
    [dispatch, pane],
  )
  return (
    <article
      ref={root}
      className="workspace-pane"
      data-widget-pane
      {...frame}
      data-focused={(focused && multi) || undefined}
      {...{ [focusedPaneAttribute]: focused || undefined }}
      aria-label={title}
      {...focusHandlers}
    >
      <header
        className="workspace-pane-header"
        // Held to the pane's top left as a drag's preview reshapes it.
        data-split-keeps="top-left"
        data-tauri-drag-region={multi ? undefined : true}
        // With more than one pane, the bar carries the pane.
        data-drag-pane={multi ? pane : undefined}
      >
        <div className="workspace-pane-name" data-shown>
          <WidgetTrail origin={origin} title={title} onOrigin={toOrigin} />
        </div>
        <span
          className="workspace-spacer"
          data-tauri-drag-region={multi ? undefined : true}
        />
        <div className="workspace-pane-actions">
          {offersWindow(answer, offeredBy(plugin)) ? (
            <IconButton
              icon="maximize"
              label="Open in Window"
              draggable={false}
              onClick={(event) => {
                event.stopPropagation()
                host.open("window")
              }}
            />
          ) : null}
          <IconButton
            icon="close"
            label="Close Pane"
            shortcut={shortcuts.shortcut("closePane")}
            draggable={false}
            data-reserved={!closable || undefined}
            tabIndex={closable ? 0 : -1}
            aria-hidden={!closable || undefined}
            onClick={(event) => {
              event.stopPropagation()
              if (closable) dispatch(closePane({ pane }))
            }}
          />
        </div>
      </header>
      <div className="workspace-pane-body" data-split-through>
        <WidgetBody
          id={widget.id}
          place="pane"
          answer={answer}
          plugin={plugin}
          host={host}
        />
      </div>
    </article>
  )
}
