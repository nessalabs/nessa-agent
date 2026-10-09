import type { HTMLAttributes, ReactNode, Ref } from "react"
import "./list-row.css"

/**
 * A row of one of the window's lists that the kit's `SidebarMenuItem` cannot
 * be: an option of a listbox (the session list, the quick switcher), or a
 * row whose second line runs under its trailing detail (the Agents overview).
 * The sidebar's rows are the kit's.
 *
 * It owns what every such row shares — its parts, how each truncates, and
 * how it answers: the pointer's fill, the press, the chosen row's fill, and
 * the weight an unread title takes. Where the parts go, how roomy the row is
 * and how large its type are the list's (`grid-template-areas` over
 * `leading`, `title`, `meta`, `description` and `trailing`, in the list's own
 * stylesheet), so two lists can differ in density and never in behaviour.
 * `shared-controls.mjs` (`rows`) measures the states on every list.
 */
export function ListRow({
  as: Element = "div",
  ref,
  leading,
  title,
  marker,
  meta,
  description,
  trailing,
  selected = false,
  unread = false,
  className,
  children,
  ...props
}: Omit<HTMLAttributes<HTMLElement>, "title"> & {
  /** A `button` for a row that is its own control; a `div` for a listbox's option. */
  as?: "div" | "button"
  ref?: Ref<HTMLElement>
  /** The agent's tile, a status glyph or an icon, before the words. */
  leading?: ReactNode
  /** What the row is, on one line, cut short with an ellipsis. */
  title: ReactNode
  /** Before the title, in its line: the unread point. */
  marker?: ReactNode
  /** A quieter detail beside the title. */
  meta?: ReactNode
  /** A second line: the last thing said. */
  description?: ReactNode
  /** A time, a glyph or a key, at the row's end. */
  trailing?: ReactNode
  /** The list's chosen row — the open one, or the one the keys rest on. */
  selected?: boolean
  /** Holds something not yet seen: the title takes the unread weight. */
  unread?: boolean
  /** Anything the row lays over itself, such as the switcher's keys. */
  children?: ReactNode
}) {
  const classes = className ? `desktop-list-row ${className}` : "desktop-list-row"
  const parts = (
    <>
      {leading ? <span className="desktop-list-row-leading">{leading}</span> : null}
      <span className="desktop-list-row-title">
        {marker}
        <span className="desktop-list-row-label">{title}</span>
      </span>
      {meta ? <span className="desktop-list-row-meta">{meta}</span> : null}
      {description ? (
        <span className="desktop-list-row-description">{description}</span>
      ) : null}
      {trailing ? <span className="desktop-list-row-trailing">{trailing}</span> : null}
      {children}
    </>
  )
  const state = {
    "data-selected": selected || undefined,
    "data-unread": unread || undefined,
  }
  return Element === "button" ? (
    <button
      type="button"
      ref={ref as Ref<HTMLButtonElement>}
      className={classes}
      {...state}
      {...props}
    >
      {parts}
    </button>
  ) : (
    <div ref={ref as Ref<HTMLDivElement>} className={classes} {...state} {...props}>
      {parts}
    </div>
  )
}
