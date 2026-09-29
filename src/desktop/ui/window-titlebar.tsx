"use client"

/** @responsibility Lays out a seamless window titlebar with stationary leading controls. */

import * as React from "react"

/** Properties of a titlebar that remains outside the collapsible shell body. */
interface WindowTitlebarProps extends React.ComponentProps<"header"> {
  /**
   * Leading controls: the sidebar toggle, then Back and Forward
   * (`history-buttons.tsx`). The titlebar owns no sidebar or history state.
   */
  leading?: React.ReactNode
  /** Controls anchored to the far end of the window. */
  trailing?: React.ReactNode
  /**
   * Physical left padding reserved for native window buttons, including their gap.
   * Defaults to 8px. Supply the host's measured inset; no platform is detected.
   * Numeric values are CSS pixels, independent of typography or theme scale.
   */
  windowControlsInset?: React.CSSProperties["paddingLeft"]
  /**
   * Height coordinated with native button geometry. Defaults to 42px.
   * Numeric values are CSS pixels; the host owns native traffic-light placement.
   */
  height?: React.CSSProperties["height"]
}

/**
 * Renders a borderless header with fixed leading controls. Place it before AppShellBody so toggling sidebars cannot move it.
 * Native dragging, window controls, theme, and navigation remain app-owned;
 * native header props are forwarded for host drag regions, refs, and events.
 * Children occupy the flexible middle region. No title or icons are invented.
 */
function WindowTitlebar({
  leading,
  trailing,
  windowControlsInset = 8,
  height = 42,
  className,
  style,
  children,
  ...props
}: WindowTitlebarProps) {
  return (
    <header
      data-slot="app-shell-titlebar"
      className={`flex w-full shrink-0 select-none items-center gap-0 border-0 bg-background pr-2 text-foreground [-webkit-user-select:none] ${className ?? ""}`}
      style={{ height, paddingLeft: windowControlsInset, ...style }}
      {...props}
    >
      {leading}
      {children ? <div className="min-w-0 flex-1">{children}</div> : null}
      {trailing ? (
        <div className="ml-auto flex shrink-0 items-center">{trailing}</div>
      ) : null}
    </header>
  )
}

export { WindowTitlebar, type WindowTitlebarProps }
