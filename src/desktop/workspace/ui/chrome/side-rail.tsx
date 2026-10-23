/**
 * Prototype: the side rail. A narrow strip at the window's left edge
 * listing the workspace and the other places beside it; choosing one gives it the whole
 * content area below the window bar. Each icon has its own small hover gesture (`side-rail.css`).
 */
import { LoadingText } from "@nessa-ui/react/loading-text"
import {
  type CSSProperties,
  type ReactElement,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react"
import { IconButton } from "../../../ui/icon-button"
import { IdentityFooter } from "./identity-footer"
import "./side-rail.css"

/** One 28px control with the window's 10px control edge on each side. */
export const SIDE_RAIL_WIDTH = 48

export interface RailItem {
  readonly id: string
  readonly name: string
  readonly icon: ReactElement
  /**
   * Whether its full view keeps "nessa Studio" — the way to Settings — in
   * the bottom-left corner, where the sidebar keeps it. On unless the item
   * needs that corner for itself.
   */
  readonly studio?: boolean
}

const stroke = {
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.6,
  strokeLinecap: "round",
  strokeLinejoin: "round",
} as const

const svg = (children: ReactNode): ReactElement => (
  <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" {...stroke}>
    {children}
  </svg>
)

export const railItems: readonly RailItem[] = [
  {
    id: "agents",
    name: "Agents",
    icon: svg(
      <>
        <path className="g-spark" d="M12 3v4M12 17v4M3 12h4M17 12h4" />
        <circle className="g-core" cx="12" cy="12" r="2.4" />
      </>,
    ),
  },
  {
    id: "notes",
    name: "Notes",
    icon: svg(
      <>
        <path d="M5 4h10l4 4v12H5z" />
        <path className="g-line" d="M8.5 12h7M8.5 15.5h4.5" />
      </>,
    ),
  },
  {
    id: "calendar",
    name: "Calendar",
    icon: svg(
      <>
        <rect x="4" y="5.5" width="16" height="14.5" rx="2.5" />
        <path className="g-page" d="M4 10h16" />
        <path className="g-ring" d="M8.5 3.5v4M15.5 3.5v4" />
      </>,
    ),
  },
  {
    id: "browser",
    name: "Browser",
    // A page reaches the window's edges; Settings stays a ⌘, away.
    studio: false,
    icon: svg(
      <>
        <circle cx="12" cy="12" r="8.5" />
        <g className="g-globe">
          <path d="M3.5 12h17M12 3.5c2.6 2.4 2.6 14.6 0 17M12 3.5c-2.6 2.4-2.6 14.6 0 17" />
        </g>
      </>,
    ),
  },
  {
    id: "files",
    name: "Files",
    icon: svg(
      <>
        <path
          className="g-back"
          d="M3.5 7.5a2 2 0 0 1 2-2h4l2 2h7a2 2 0 0 1 2 2V18a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z"
        />
        <path className="g-lid" d="M3.5 11h17" />
      </>,
    ),
  },
]

export function SideRail({
  open,
  active,
  onPick,
}: {
  open: boolean
  active: string
  onPick: (id: string) => void
}) {
  const index = Math.max(
    0,
    railItems.findIndex((item) => item.id === active),
  )
  const nav = useRef<HTMLElement>(null)
  // Folding away — for room, or by the toggle — never strands the keyboard on
  // a hidden button: focus goes to the toggle in the bottom corner, or else lets go.
  useLayoutEffect(() => {
    const rail = nav.current
    if (open || !rail?.contains(document.activeElement)) return
    rail.parentElement?.querySelector<HTMLElement>(".side-rail-toggle")?.focus()
    if (rail.contains(document.activeElement))
      (document.activeElement as HTMLElement).blur()
  }, [open])
  return (
    <nav
      ref={nav}
      className="side-rail"
      data-open={open || undefined}
      id="side-rail"
      aria-label="Side Rail"
      aria-hidden={!open || undefined}
      style={{ "--rail-index": index } as CSSProperties}
    >
      <div className="side-rail-items">
        <span className="side-rail-lens" aria-hidden="true" />
        {railItems.map((item, i) => (
          <IconButton
            key={item.id}
            icon={item.icon}
            label={item.name}
            // Beside the icon, toward what it opens; never over its neighbours.
            tooltipSide="right"
            className="side-rail-item"
            data-rail-item={item.id}
            aria-current={item.id === active ? "page" : undefined}
            tabIndex={open ? 0 : -1}
            style={{ "--i": i } as CSSProperties}
            onClick={() => onPick(item.id)}
          />
        ))}
      </div>
    </nav>
  )
}

/** Shows or hides the side rail from its slot at the window's bottom-left. */
export function SideRailButton({
  open,
  room,
  toggle,
}: {
  open: boolean
  /** Whether the window has room for the rail beside what is open (`railFits`). */
  room: boolean
  toggle: () => void
}) {
  const action = room
    ? `${open ? "Hide" : "Show"} Side Rail`
    : "No Room for the Side Rail"
  return (
    <IconButton
      icon={svg(
        <>
          <rect className="g-a" x="4" y="4" width="6.5" height="6.5" rx="1.8" />
          <rect className="g-b" x="13.5" y="4" width="6.5" height="6.5" rx="1.8" />
          <rect className="g-c" x="4" y="13.5" width="6.5" height="6.5" rx="1.8" />
          <rect className="g-d" x="13.5" y="13.5" width="6.5" height="6.5" rx="1.8" />
        </>,
      )}
      label={action}
      tooltipSide="above"
      className="side-rail-toggle"
      aria-expanded={open}
      aria-controls="side-rail"
      // Said, not hidden: the window is too narrow for it beside what is open.
      aria-disabled={!room || undefined}
      onClick={room ? toggle : undefined}
    />
  )
}

/**
 * How long an item's view may stay empty while its content loads before a
 * loading animation says it is still coming. Most loads finish well inside
 * it, and a loader that flashes for a moment reads as jitter.
 */
export const RAIL_LOADING_DELAY_MS = 3000

/**
 * An item's full-window view. It covers the workspace from its first frame —
 * no fade that lets the panes show through — and stays empty while the item
 * loads; only a load still going after `RAIL_LOADING_DELAY_MS` shows the
 * loading animation. The items have no content yet, so for now that is what
 * each one shows.
 */
export function RailView({ item }: { item: RailItem }) {
  return (
    <section key={item.id} className="rail-view" aria-label={item.name}>
      <RailLoading name={item.name} />
      {item.studio === false ? null : (
        <div className="rail-view-foot">
          <IdentityFooter />
        </div>
      )}
    </section>
  )
}

/** Nothing, then the kit's loading text once the load runs long. */
function RailLoading({ name }: { name: string }) {
  const [long, setLong] = useState(false)
  useEffect(() => {
    const timer = window.setTimeout(() => setLong(true), RAIL_LOADING_DELAY_MS)
    return () => window.clearTimeout(timer)
  }, [])
  if (!long) return null
  return <LoadingText className="rail-view-loading">{`Loading ${name}`}</LoadingText>
}
