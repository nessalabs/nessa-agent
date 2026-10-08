import type { ReactNode } from "react"
import { DesktopIcon } from "./icons"
import { tooltip, type TooltipAttributes } from "./tooltip"
import "./identity.css"

/**
 * The window's name, "nessa Studio", as the one control every layout puts at
 * its sidebar's foot to open Settings — and, in Settings, "‹ nessa Agent" in
 * the same place, the way back. One pill of the corner controls' size,
 * filled under the pointer, its product word quieter until then.
 * `shared-controls.mjs` (`identity`) measures it in each place.
 */
export function IdentityButton({
  product,
  back = false,
  label,
  shortcut,
  onClick,
}: {
  /** The word after "nessa": where the control is, or where it goes. */
  product: string
  /** Leads with a chevron: the control goes back to where it was opened from. */
  back?: boolean
  /** Its accessible name, which says what it does. */
  label: string
  /** The chord that does the same; with one, the control says "Settings" and the chord in its tooltip. */
  shortcut?: string
  onClick: () => void
}) {
  const hint: TooltipAttributes | Record<string, never> = shortcut
    ? tooltip("Settings", { shortcut })
    : {}
  return (
    <button
      type="button"
      className="desktop-identity-button"
      data-back={back || undefined}
      aria-label={label}
      {...hint}
      onClick={onClick}
    >
      {back ? <DesktopIcon name="chevronLeft" className="desktop-identity-back" /> : null}
      <span className="desktop-identity-words">
        <b>nessa</b> <span>{product}</span>
      </span>
    </button>
  )
}

/**
 * The row that holds the name at a sidebar's foot: one corner control tall,
 * the control inset from the card's side as the titlebar's controls are
 * from its top. `className` is a hook for the surface that places it.
 */
export function IdentityRow({
  as: Element = "footer",
  className,
  children,
}: {
  /** `div` where the row sits inside another landmark, as Settings' does in its nav. */
  as?: "footer" | "div"
  className?: string
  children: ReactNode
}) {
  return (
    <Element className={className ? `desktop-identity ${className}` : "desktop-identity"}>
      {children}
    </Element>
  )
}
