import { memo, useCallback, useRef } from "react"
import {
  WidgetAnswerOf,
  WidgetBody,
  widgetOrigin,
  widgetTitle,
  type EscapeStack,
  type WidgetAnswer,
  type WidgetPlugin,
  type WidgetRef,
} from "../../../widgets"
import { widgetWindowAttribute } from "../../adapters/dom/focus"
import { useEscapeScope, windowScope } from "../../adapters/dom/widget-escape"
import { openSession } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectWindowWidget } from "../../adapters/store/selectors"
import { useWindowWidgetHost } from "../../adapters/store/widget-hosts"
import { paneItemKey, widgetItem } from "../../model/pane-item"
import { IconButton } from "../../../ui/icon-button"
import { useWorkspaceFrame } from "../workspace-frame"
import { PaneHeaderFrame } from "./pane-header-frame"
import { WidgetTrail } from "./widget-trail"

/**
 * The window (ADR 326): the content region's third view, a widget drawn over
 * the chat area — the panes stay laid out beneath it, unseen and out of
 * reach — while the session list stays in reach beside it. It is left as the
 * overview is: its close, Escape (`adapters/dom/widget-escape.ts`), a session
 * chosen, any change of the panes; ⌘W closes it, never a pane beneath it
 * (`ui/layouts/widgets.test.tsx`, `widgets.mjs --only window-left`).
 *
 * A widget asked for over another replaces it: each is drawn under its own
 * key, so its view, its steps back and the caret in it go with it.
 */
export const WidgetWindow = memo(function WidgetWindow() {
  const widget = useWorkspaceSelector(selectWindowWidget)
  if (!widget) return null
  return <WindowWidget key={paneItemKey(widgetItem(widget))} widget={widget} />
})

function WindowWidget({ widget }: { widget: WidgetRef }) {
  const root = useRef<HTMLElement>(null)
  const steps = useEscapeScope(windowScope, root)
  return (
    <WidgetAnswerOf widget={widget}>
      {(answer, plugin) => (
        <section
          ref={root}
          className="workspace-widget-window"
          {...{ [widgetWindowAttribute]: "" }}
          aria-label={widgetTitle(answer)}
        >
          <WindowParts steps={steps} widget={widget} answer={answer} plugin={plugin} />
        </section>
      )}
    </WidgetAnswerOf>
  )
}

function WindowParts({
  steps,
  widget,
  answer,
  plugin,
}: {
  steps: EscapeStack
  widget: WidgetRef
  answer: WidgetAnswer
  plugin: WidgetPlugin | undefined
}) {
  const dispatch = useWorkspaceDispatch()
  const shortcuts = useWorkspaceFrame()
  const origin = widgetOrigin(answer)
  const host = useWindowWidgetHost(widget, origin, steps)
  // Back to the panes, focusing the pane showing the conversation, or
  // opening it in the focused pane.
  const toOrigin = useCallback(
    (sessionId: string) => dispatch(openSession({ sessionId })),
    [dispatch],
  )
  return (
    <>
      {/* Alone in the titlebar's row, as one pane is: the bar moves the window. */}
      <PaneHeaderFrame
        pane={null}
        multi={false}
        name={
          <WidgetTrail origin={origin} title={widgetTitle(answer)} onOrigin={toOrigin} />
        }
        actions={
          <IconButton
            icon="close"
            label="Close"
            shortcut={shortcuts.shortcut("closePane")}
            onClick={host.close}
          />
        }
      />
      <div className="workspace-pane-body">
        <WidgetBody
          id={widget.id}
          place="window"
          answer={answer}
          plugin={plugin}
          host={host}
        />
      </div>
    </>
  )
}
