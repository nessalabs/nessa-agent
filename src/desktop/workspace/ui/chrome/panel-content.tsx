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
import {
  closePanelTab,
  openSession,
  openWidget,
  showPanelTab,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChatPanel, selectFocusedSessionId } from "../../adapters/store/selectors"
import { usePanelWidgetHost } from "../../adapters/store/widget-hosts"
import { paneItemKey, widgetItem } from "../../model/pane-item"
import { isChatContextTab, panelTabs } from "../../model/chat-context"
import { ChatContext } from "./chat-context"
import { IconButton } from "../../../ui/icon-button"
import { WidgetTrail } from "../panes/widget-trail"

/** A tab's id: the widget's item key, one-to-one with the widget (`pane-item.ts`). */
const tabId = (widget: WidgetRef): string => paneItemKey(widgetItem(widget))

/**
 * What the right panel shows: the focused chat's own panel (ADR 326's
 * `panel` place). Its first tab is always Chat context (`chat-context.ts`),
 * then the widgets opened in it; with none opened, Chat context alone, and
 * no tab row. With no chat in front, a note saying so.
 */
export const PanelContent = memo(function PanelContent() {
  const sessionId = useWorkspaceSelector(selectFocusedSessionId)
  const panel = useWorkspaceSelector(
    (state) => (sessionId === null ? null : selectChatPanel(state, sessionId)),
    shallowEqual,
  )
  if (sessionId === null)
    return (
      <div className="workspace-panel-body" data-panel-empty>
        <EmptyState
          variant="compact"
          className="desktop-empty-note"
          title="No chat open"
        />
      </div>
    )
  const { tabs, front, widgets } = panelTabs(sessionId, panel)
  return (
    <>
      {widgets.length > 0 ? (
        <PanelTabs sessionId={sessionId} tabs={tabs} front={front} />
      ) : null}
      {isChatContextTab(front) ? (
        <div
          className="workspace-panel-context"
          data-tabbed={widgets.length > 0 || undefined}
          role={widgets.length > 0 ? "tabpanel" : undefined}
          id={widgets.length > 0 ? `chat-tab-panel-${tabId(front)}` : undefined}
          aria-labelledby={widgets.length > 0 ? `chat-tab-${tabId(front)}` : undefined}
        >
          <ChatContext sessionId={sessionId} />
        </div>
      ) : (
        // Under the tab in front's key: another tab is drawn afresh, its
        // view, its steps back and the caret in it never the last one's.
        <PanelWidget key={tabId(front)} sessionId={sessionId} widget={front} />
      )}
    </>
  )
})

/**
 * The panel's tabs, in its top row: Chat context, which stays, then one per
 * widget open in it, each closeable.
 */
function PanelTabs({
  sessionId,
  tabs,
  front,
}: {
  sessionId: string
  tabs: readonly WidgetRef[]
  front: WidgetRef
}) {
  const dispatch = useWorkspaceDispatch()
  const byId = new Map(tabs.map((widget) => [tabId(widget), widget]))
  const show = (id: string) => {
    const widget = byId.get(id)
    if (!widget) return
    // Chat context is stored as a tab only once it is chosen (`chat-context.ts`).
    if (isChatContextTab(widget))
      dispatch(openWidget({ widget, place: "panel", origin: sessionId }))
    else dispatch(showPanelTab({ sessionId, widget }))
  }
  const close = (id: string) => {
    const widget = byId.get(id)
    if (widget && !isChatContextTab(widget))
      dispatch(closePanelTab({ sessionId, widget }))
  }
  const widgets = tabs.filter((widget) => !isChatContextTab(widget))
  return (
    <div className="workspace-panel-tabs" data-tauri-drag-region>
      <WithTitles widgets={widgets}>
        {(titles) => (
          <ChatTabs
            label="Panel"
            // The window's top row, over the header picture: plain tabs, the
            // one in front opaque.
            variant="plain"
            tabs={tabs.map((widget): ChatTabItem => {
              const context = isChatContextTab(widget)
              return {
                id: tabId(widget),
                title: context ? "Chat context" : titles[widgets.indexOf(widget)],
                kind: "file",
                icon: (
                  <DesktopIcon name={context ? "chatContext" : "panelRight"} size={16} />
                ),
                closeable: !context,
              }
            })}
            value={tabId(front)}
            onValueChange={show}
            onClose={close}
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
