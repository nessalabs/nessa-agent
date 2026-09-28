import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  type ReactNode,
} from "react"
import { DesktopIcon } from "../../../ui/icons"
import { tooltip } from "../../../ui/tooltip"
import {
  focusComposer,
  useWorkspaceDispatch,
  useWorkspaceSelector,
} from "../../../workspace"
import { labelOf, useKeyBindings } from "../../../workspace/adapters/dom/shortcuts"
import { useAgentsOverviewPreference } from "../adapters/preference"
import {
  selectContentView,
  selectWaitingCount,
  showContent,
} from "../adapters/workspace-bridge"
import { AgentsOverview } from "./agents-overview"
import { toggleKeys } from "./overview-keys"
import "./agents-overview.css"

/**
 * Whether the agents overview is on in Settings, and whether it holds the
 * content region now. One per workspace layout, around its shell, so the
 * sidebar's entry, the shell and the chat area read the same answer.
 */
interface OverviewPlace {
  readonly enabled: boolean
  readonly shown: boolean
  readonly show: (shown: boolean) => void
}

const PlaceContext = createContext<OverviewPlace>({
  enabled: false,
  shown: false,
  show: () => {},
})

/**
 * Where the overview is for a layout's window: the workspace's content view
 * (`showContent`), so going anywhere else — a channel, a session, a status
 * view, ⌘N, an agent's dispatch — goes back to the panes by the workspace's
 * one rule (`navigated`). ⌘0 goes to the overview, and back, while the
 * experiment is on; turned off, the panes come back.
 */
export function AgentsOverviewScope({ children }: { children: ReactNode }) {
  const [flag] = useAgentsOverviewPreference()
  const enabled = flag === "on"
  const dispatch = useWorkspaceDispatch()
  const content = useWorkspaceSelector(selectContentView)
  useEffect(() => {
    if (!enabled && content === "agents") dispatch(showContent({ content: "panes" }))
  }, [enabled, content, dispatch])
  useKeyBindings(toggleKeys, () => {
    if (!enabled) return false
    dispatch(showContent({ content: content === "agents" ? "panes" : "agents" }))
  })
  const place = useMemo<OverviewPlace>(
    () => ({
      enabled,
      shown: enabled && content === "agents",
      show: (shown) => dispatch(showContent({ content: shown ? "agents" : "panes" })),
    }),
    [enabled, content, dispatch],
  )
  return <PlaceContext.Provider value={place}>{children}</PlaceContext.Provider>
}

/**
 * Whether the overview is on in Settings: the sidebar then offers "Agents"
 * in place of its "Needs you" and "Running" views, which the overview holds.
 */
export function useAgentsOverviewEnabled(): boolean {
  return useContext(PlaceContext).enabled
}

/**
 * Whether the overview holds the workspace's content region now: the shell
 * asks, to set the session list aside while it does.
 */
export function useAgentsOverviewShown(): boolean {
  return useContext(PlaceContext).shown
}

/**
 * The chat area, with the overview over it while shown. The panes stay
 * mounted and laid out underneath — inert and faded — so their room, scroll
 * and caret are as they were when the person comes back. With the
 * experiment off this draws nothing of its own.
 */
export function AgentsOverviewArea({ children }: { children: ReactNode }) {
  const { shown, show } = useContext(PlaceContext)
  const under = useRef<HTMLDivElement>(null)
  // The chat area's width as it was laid out before the overview came —
  // at least the overview's own, which only grows as the list is set aside —
  // read while the page is still as it was, so the overview is drawn in its
  // arrangement the first time rather than laid out twice.
  const hint = useRef(0)
  const was = useRef(shown)
  if (shown && !was.current) hint.current = under.current?.offsetWidth ?? 0
  was.current = shown
  const leave = useCallback(() => {
    show(false)
    // Back to the panes: the caret returns to the focused pane's composer.
    focusComposer(under.current ?? document)
  }, [show])
  return (
    <div className="agents-overview-area" data-shown={shown || undefined}>
      <div className="agents-overview-under" ref={under} inert={shown || undefined}>
        {children}
      </div>
      {shown ? <AgentsOverview onLeave={leave} widthHint={hint.current} /> : null}
    </div>
  )
}

/**
 * The sidebar's way in: "Agents", with how many wait on the person, in place
 * of the "Needs you" and "Running" views. Absent while the experiment is off.
 */
export function AgentsOverviewEntry() {
  const { enabled, shown, show } = useContext(PlaceContext)
  const waiting = useWorkspaceSelector(selectWaitingCount)
  if (!enabled) return null
  return (
    <button
      type="button"
      className="workspace-row agents-overview-entry"
      data-active={shown || undefined}
      aria-current={shown ? "page" : undefined}
      {...tooltip("Every agent at a glance", {
        shortcut: labelOf(toggleKeys, "toggle"),
      })}
      // A place to go, like a channel: pressed again, it stays.
      onClick={() => show(true)}
    >
      <span className="workspace-row-icon" aria-hidden="true">
        <DesktopIcon name="workspace" />
      </span>
      <span className="workspace-truncate">Agents</span>
      {waiting > 0 ? (
        <span
          className="workspace-badge"
          data-tone="needs"
          aria-label={`${waiting} ${waiting === 1 ? "needs" : "need"} you`}
        >
          {waiting}
        </span>
      ) : null}
    </button>
  )
}
