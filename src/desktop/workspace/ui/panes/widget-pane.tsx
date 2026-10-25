import { memo, useCallback, useRef, useState, type RefObject } from "react"
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
  type WidgetCrumb,
  type WidgetRef,
} from "../../../widgets"
import { paneScope, useEscapeScope } from "../../adapters/dom/widget-escape"
import { openBeside } from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"
import { PaneFocusMark } from "./pane-focus-mark"
import { usePaneWidgetHost } from "../../adapters/store/widget-hosts"
import type { PaneFrame } from "../../../split-panes"
import type { PaneKey } from "../../../split-panes/model/pane-layout"
import { IconButton } from "../../../ui/icon-button"
import { ClosePaneButton, PaneHeaderFrame } from "./pane-header-frame"
import { useFilledAfterFirstPaint } from "./use-filled-after-first-paint"
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
  const focusHandlers = usePaneFocus(pane)
  const origin = widgetOrigin(answer)
  const title = widgetTitle(answer)
  const [crumb, setCrumb] = useState<WidgetCrumb | null>(null)
  const host = usePaneWidgetHost(pane, widget, origin, steps, setCrumb)
  const filled = useFilledAfterFirstPaint()
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
      aria-label={title}
      {...focusHandlers}
    >
      <PaneFocusMark paneKey={pane} multi={multi} />
      <PaneHeaderFrame
        pane={pane}
        multi={multi}
        name={
          <WidgetTrail origin={origin} title={title} onOrigin={toOrigin} inner={crumb} />
        }
        actions={
          <>
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
            <ClosePaneButton pane={pane} />
          </>
        }
      />
      <div className="workspace-pane-body" data-split-through>
        {filled ? (
          <WidgetBody
            id={widget.id}
            place="pane"
            answer={answer}
            plugin={plugin}
            host={host}
          />
        ) : null}
      </div>
    </article>
  )
}
