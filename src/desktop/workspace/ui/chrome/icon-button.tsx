import { forwardRef, type ButtonHTMLAttributes } from "react"
import { DesktopIcon, type DesktopIconRole } from "../../../ui/icons"

/**
 * A square control with one icon, named for what it does. The shortcut, when
 * there is one, is part of its name and tooltip: "Close Pane (⌘W)".
 */
export const IconButton = forwardRef<
  HTMLButtonElement,
  Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children"> & {
    icon: DesktopIconRole
    label: string
    shortcut?: string
  }
>(function IconButton({ icon, label, shortcut, className, ...props }, ref) {
  const name = shortcut ? `${label} (${shortcut})` : label
  return (
    <button
      ref={ref}
      type="button"
      className={
        className ? `workspace-icon-button ${className}` : "workspace-icon-button"
      }
      aria-label={name}
      title={name}
      {...props}
    >
      <DesktopIcon name={icon} />
    </button>
  )
})
