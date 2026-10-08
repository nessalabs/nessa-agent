import {
  forwardRef,
  type ButtonHTMLAttributes,
  type ComponentProps,
  type ElementType,
} from "react"
import { DesktopIcon, type DesktopIconRole } from "./icons"
import type { TooltipSide } from "../model/tooltip-placement"
import { tooltip } from "./tooltip"
import "./icon-button.css"

/** The box: 26 (a footer's), 28 (the window's controls), 32 (a roomy toolbar's). */
export type IconButtonSize = "sm" | "md" | "lg"
/** `rounded` is concentric with the pane's corner; `pill` is a capsule. */
export type IconButtonShape = "rounded" | "pill"
/** How loud it rests: `muted` for chrome, `faint` for the quietest, `ink` over a picture. */
export type IconButtonTone = "muted" | "faint" | "ink"

/**
 * The window's one icon-only control, named for what it does. The shortcut,
 * when there is one, is part of its name and its tooltip, said one way
 * everywhere: "Close Pane (⌘W)". Size, shape and tone are the only things that
 * differ between the window's icon buttons; a surface that wants more asks
 * here rather than styling its own.
 *
 * `as` renders another button-shaped component — the kit's sidebar toggle —
 * in this one's clothes.
 */
export const IconButton = forwardRef<
  HTMLButtonElement,
  Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children"> & {
    icon: DesktopIconRole
    label: string
    shortcut?: string
    size?: IconButtonSize
    shape?: IconButtonShape
    tone?: IconButtonTone
    /** Which side its tooltip prefers: below, unless it sits low on the window. */
    tooltipSide?: TooltipSide
    as?: ElementType<ComponentProps<"button">>
  }
>(function IconButton(
  {
    icon,
    label,
    shortcut,
    size = "md",
    shape = "rounded",
    tone = "muted",
    tooltipSide,
    as: Element = "button",
    className,
    ...props
  },
  ref,
) {
  return (
    <Element
      ref={ref}
      type="button"
      className={className ? `desktop-icon-button ${className}` : "desktop-icon-button"}
      data-size={size}
      data-shape={shape}
      data-tone={tone}
      aria-label={shortcut ? `${label} (${shortcut})` : label}
      {...tooltip(label, { shortcut, side: tooltipSide })}
      {...props}
    >
      <DesktopIcon name={icon} />
    </Element>
  )
})
