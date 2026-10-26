import {
  memo,
  useCallback,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from "react"
import { shallowEqual } from "react-redux"
import { ChatTabs, type ChatTabItem } from "@nessa-ui/react/chat-tabs"
import { EmptyState } from "@nessa-ui/react/empty-state"
import {
  offeredBy,
  offersWindow,
  WidgetAnswerOf,
  WidgetBody,
  widgetOrigin,
  widgetTitle,
  type EscapeStack,
  type WidgetAnswer,
  type WidgetCrumb,
  type WidgetPlugin,
  type WidgetRef,
} from "../../../widgets"
import { DesktopIcon } from "../../../ui/icons"
import { panelWidgetAttribute } from "../../adapters/dom/focus"
import { panelScope, useEscapeScope } from "../../adapters/dom/widget-escape"
import { closePanelTab, openSession, showPanelTab } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChatPanel, selectFocusedSessionId } from "../../adapters/store/selectors"
import { usePanelWidgetHost } from "../../adapters/store/widget-hosts"
import { paneItemKey, widgetItem } from "../../model/pane-item"
import type { ChatPanel } from "../../model/chat-panel"
import { IconButton } from "../../../ui/icon-button"
import { WidgetTrail } from "../panes/widget-trail"

/** A tab's id: the widget's item key, one-to-one with the widget (`pane-item.ts`). */
const tabId = (widget: WidgetRef): string => paneItemKey(widgetItem(widget))

/**
 * What the right panel shows: the focused chat's own panel (ADR 326's
 * `panel` place) — its widgets as tabs across the panel's top row, the one
 * in front drawn below — or, with none open, a note saying so. Chats have no
 * tabs; only what opens in the panel does.
 */
export const PanelContent = memo(function PanelContent() {
  const sessionId = useWorkspaceSelector(selectFocusedSessionId)
  const panel = useWorkspaceSelector(
    (state) => (sessionId === null ? null : selectChatPanel(state, sessionId)),
    shallowEqual,
  )
  if (sessionId === null || !panel)
    return (
      <div className="workspace-panel-body" data-panel-empty>
        <EmptyState
          variant="compact"
          className="desktop-empty-note"
          title="Nothing open"
        />
      </div>
    )
  return (
    <>
      <PanelTabs sessionId={sessionId} panel={panel} />
      {/* Under the tab in front's key: another tab is drawn afresh, its
          view, its steps back and the caret in it never the last one's. */}
      <PanelWidget key={tabId(panel.front)} sessionId={sessionId} widget={panel.front} />
    </>
  )
})

/** The panel's tabs, in its top row: one per widget open in it, each closeable. */
function PanelTabs({ sessionId, panel }: { sessionId: string; panel: ChatPanel }) {
  const dispatch = useWorkspaceDispatch()
  const byId = new Map(panel.tabs.map((widget) => [tabId(widget), widget]))
  const act = (id: string, change: typeof showPanelTab | typeof closePanelTab) => {
    const widget = byId.get(id)
    if (widget) dispatch(change({ sessionId, widget }))
  }
  return (
    <div className="workspace-panel-tabs" data-tauri-drag-region>
      <WithTitles widgets={panel.tabs}>
        {(titles) => (
          <ChatTabs
            label="Panel"
            // The window's top row, over the header picture: plain tabs, the
            // one in front opaque.
            variant="plain"
            tabs={panel.tabs.map((widget, at): ChatTabItem => ({
              id: tabId(widget),
              title: titles[at],
              kind: "file",
              icon: <DesktopIcon name="panelRight" size={16} />,
              closeable: true,
            }))}
            value={tabId(panel.front)}
            onValueChange={(id) => act(id, showPanelTab)}
            onClose={(id) => act(id, closePanelTab)}
          />
        )}
      </WithTitles>
    </div>
  )
}

/**
 * Each widget's title as its plugin names it (`widgetTitle`), one answer at a
 * time: every widget's answer is its own subscription (`WidgetAnswerOf`),
 * each held under its widget's key, so a tab closed or opened re-reads only
 * what moved.
 */
function WithTitles({
  widgets,
  titles = [],
  children,
}: {
  widgets: readonly WidgetRef[]
  titles?: readonly string[]
  children: (titles: readonly string[]) => ReactNode
}) {
  const next = widgets[titles.length]
  if (!next) return children(titles)
  return (
    <WidgetAnswerOf key={tabId(next)} widget={next}>
      {(answer) => (
        <WithTitles widgets={widgets} titles={[...titles, widgetTitle(answer)]}>
          {children}
        </WithTitles>
      )}
    </WidgetAnswerOf>
  )
}

/**
 * The tab in front: its widget's trail — the way back to the conversation it
 * belongs to — and, where the plugin draws one, the window (expand); then the
 * widget's host draws the rest (`WidgetBody`). Its view's steps back answer
 * Escape while focus is in the panel.
 */
const PanelWidget = memo(function PanelWidget({
  sessionId,
  widget,
}: {
  sessionId: string
  widget: WidgetRef
}) {
  const root = useRef<HTMLDivElement>(null)
  const steps = useEscapeScope(panelScope, root)
  return (
    <WidgetAnswerOf widget={widget}>
      {(answer, plugin) => (
        <PanelWidgetParts
          root={root}
          steps={steps}
          sessionId={sessionId}
          widget={widget}
          answer={answer}
          plugin={plugin}
        />
      )}
    </WidgetAnswerOf>
  )
})

function PanelWidgetParts({
  root,
  steps,
  sessionId,
  widget,
  answer,
  plugin,
}: {
  root: RefObject<HTMLDivElement | null>
  steps: EscapeStack
  sessionId: string
  widget: WidgetRef
  answer: WidgetAnswer
  plugin: WidgetPlugin | undefined
}) {
  const dispatch = useWorkspaceDispatch()
  const origin = widgetOrigin(answer)
  const title = widgetTitle(answer)
  const [crumb, setCrumb] = useState<WidgetCrumb | null>(null)
  const host = usePanelWidgetHost(sessionId, widget, origin, steps, setCrumb)
  // Back to the conversation: focused where it is, or shown in the focused pane.
  const toOrigin = useCallback(
    (origin: string) => dispatch(openSession({ sessionId: origin })),
    [dispatch],
  )
  return (
    <div
      ref={root}
      className="workspace-panel-widget"
      {...{ [panelWidgetAttribute]: tabId(widget) }}
      role="tabpanel"
      // Paired with its tab (`ChatTabs` names each tab's panel so).
      id={`chat-tab-panel-${tabId(widget)}`}
      aria-labelledby={`chat-tab-${tabId(widget)}`}
    >
      <div className="workspace-panel-widget-head">
        <WidgetTrail
          origin={origin === sessionId ? undefined : origin}
          title={title}
          onOrigin={toOrigin}
          inner={crumb}
        />
        {offersWindow(answer, offeredBy(plugin)) ? (
          <IconButton
            icon="maximize"
            label="Open in Window"
            onClick={() => host.open("window")}
          />
        ) : null}
      </div>
      <div className="workspace-panel-widget-body">
        <WidgetBody
          id={widget.id}
          place="panel"
          answer={answer}
          plugin={plugin}
          host={host}
        />
      </div>
    </div>
  )
}
