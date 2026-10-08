import type { HTMLAttributes, ReactNode, Ref } from "react"
import { closePane } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectPaneClosable } from "../../adapters/store/selectors"
import type { PaneKey } from "../../../split-panes/model/pane-layout"
import { IconButton } from "../../../ui/icon-button"
import { useWorkspaceFrame } from "../workspace-frame"
import { headerBar } from "./header-bar"

/**
 * The bar atop a session's pane, a widget's pane and the window: its name at
 * the start, a spacer that moves the window or carries the pane
 * (`headerBar`), what a session's widgets add, and its actions at the end.
 * The three headers differ only in what they put in it.
 */
export function PaneHeaderFrame({
  pane,
  multi,
  name,
  nameShown = true,
  accessories,
  actions,
  ref,
  ...props
}: HTMLAttributes<HTMLElement> & {
  /** The pane the bar heads; none for the window. */
  pane: PaneKey | null
  multi: boolean
  /** What the bar is: a session's mark, title and state, or a widget's trail. */
  name: ReactNode
  /** Whether the name shows; hidden, it keeps its place and says nothing. */
  nameShown?: boolean
  /** What sits between the spacer and the actions: a session's widgets, a passing note. */
  accessories?: ReactNode
  actions: ReactNode
  ref?: Ref<HTMLElement>
}) {
  const bar = headerBar({ pane, multi })
  return (
    <header ref={ref} className="workspace-pane-header" {...bar.bar} {...props}>
      <div
        className="workspace-pane-name"
        data-shown={nameShown || undefined}
        aria-hidden={nameShown ? undefined : true}
      >
        {name}
      </div>
      <span className="workspace-spacer" {...bar.spacer} />
      {accessories}
      <div className="workspace-pane-actions">{actions}</div>
    </header>
  )
}

/**
 * A pane's ×. It keeps its room when there is nothing to close — a new
 * session's home alone — so the controls beside it stay put (`data-reserved`,
 * `ui/icon-button.css`; `layouts.test.tsx` holds it).
 */
export function ClosePaneButton({ pane }: { pane: PaneKey }) {
  const dispatch = useWorkspaceDispatch()
  const frame = useWorkspaceFrame()
  const closable = useWorkspaceSelector((state) => selectPaneClosable(state, pane))
  return (
    <IconButton
      icon="close"
      label="Close Pane"
      shortcut={frame.shortcut("closePane")}
      draggable={false}
      data-reserved={!closable || undefined}
      tabIndex={closable ? 0 : -1}
      aria-hidden={!closable || undefined}
      onClick={(event) => {
        event.stopPropagation()
        if (closable) dispatch(closePane({ pane }))
      }}
    />
  )
}
