import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { DesktopIcon } from "../../../ui/icons"
import { tooltip } from "../../../ui/tooltip"
import { focusComposer, useWorkspaceSelector } from "../../../workspace"
import { labelOf, useKeyBindings } from "../../../workspace/adapters/dom/shortcuts"
import { useAgentsOverviewPreference } from "../adapters/preference"
import { selectFocusedSessionId, selectWaitingCount } from "../adapters/workspace-bridge"
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
 * Holds where the overview is for a layout's window: shown or not, and put
 * away whenever the workspace moves to another session — a row, the
 * switcher, ⌘N, an agent's dispatch — so the chat area always shows what was
 * asked for. ⌘0 opens and closes it while the experiment is on.
 */
export function AgentsOverviewScope({ children }: { children: ReactNode }) {
  const [flag] = useAgentsOverviewPreference()
  const enabled = flag === "on"
  const [open, setOpen] = useState(false)
  const focused = useWorkspaceSelector(selectFocusedSessionId)
  const seen = useRef(focused)
  useEffect(() => {
    if (seen.current === focused) return
    seen.current = focused
    setOpen(false)
  }, [focused])
  useKeyBindings(toggleKeys, () => {
    if (!enabled) return false
    setOpen((was) => !was)
  })
  const place = useMemo<OverviewPlace>(
    () => ({ enabled, shown: enabled && open, show: setOpen }),
    [enabled, open],
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
      {shown ? <AgentsOverview onLeave={leave} /> : null}
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
      aria-pressed={shown}
      {...tooltip("Every agent at a glance", {
        shortcut: labelOf(toggleKeys, "toggle"),
      })}
      onClick={() => show(!shown)}
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
