/**
 * Prototype: the side rail. A narrow strip at the window's left edge
 * listing the workspace and the other places beside it; choosing one gives it the whole
 * content area below the window bar. Each icon has its own small hover gesture (`side-rail.css`).
 */
import { LoadingText } from "@nessa-ui/react/loading-text"
import {
  createContext,
  type CSSProperties,
  type ReactElement,
  type ReactNode,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react"
import { IconButton } from "../../../ui/icon-button"
import "./side-rail.css"

/** One 28px control on the sidebar's icon column (`--rail-column`, 18px), with as much on its other side. */
export const SIDE_RAIL_WIDTH = 64

export interface RailItem {
  readonly id: string
  readonly name: string
  readonly icon: ReactElement
  /**
   * Whether its full view keeps "nessa Studio" — the way to Settings — in
   * the bottom-left corner, where the sidebar keeps it. On unless the item's
   * view wants that corner clear.
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
    id: "devices",
    name: "Devices",
    // Its cards run to the window's foot, under the corner.
    studio: false,
    icon: svg(
      <>
        <rect className="g-screen" x="2.5" y="4.5" width="13" height="10" rx="1.8" />
        <path d="M6.5 18.5h5M9 14.5v4" />
        <rect className="g-phone" x="17.5" y="8" width="4" height="11" rx="1.2" />
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
      <span className="side-rail-pill" aria-hidden="true" />
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
 * What each item's view shows, by item id, given by the window's
 * composition: the workspace draws the rail and knows no item's content.
 */
const RailContent = createContext<Readonly<Record<string, ReactNode>>>({})

export function RailContentProvider({
  views,
  children,
}: {
  views: Readonly<Record<string, ReactNode>>
  children: ReactNode
}) {
  return <RailContent.Provider value={views}>{children}</RailContent.Provider>
}

/**
 * An item's full-window view. It covers the workspace from its first frame —
 * no fade that lets the panes show through — and shows the content the
 * composition gave for the item. One with none stays empty while it loads;
 * only a load still going after `RAIL_LOADING_DELAY_MS` shows the loading
 * animation.
 */
export function RailView({ item }: { item: RailItem }) {
  const content = useContext(RailContent)[item.id]
  return (
    <section
      key={item.id}
      className="rail-view"
      aria-label={item.name}
      data-rail-view-item={item.id}
    >
      {content ?? <RailLoading name={item.name} />}
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
