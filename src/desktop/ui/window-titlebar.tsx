"use client"

/** @responsibility Lays out a seamless window titlebar with stationary leading controls and application-owned history actions. */

import * as React from "react"

import { Button } from "@nessa-ui/react/button"

/** One application-owned history action; icons are supplied by the consumer. */
interface WindowTitlebarAction {
  /** Accessible button name, also used as its tooltip. */
  label: string
  /** Visible icon; the titlebar hides it from the accessibility tree. */
  icon: React.ReactNode
  /** Whether history permits this action. Defaults to false. */
  disabled?: boolean
  /** Requests navigation; the titlebar never reads or changes browser history. */
  onClick?: React.MouseEventHandler<HTMLButtonElement>
}

/** Properties of a titlebar that remains outside the collapsible shell body. */
interface WindowTitlebarProps extends React.ComponentProps<"header"> {
  /** Leading controls, usually a SidebarTrigger. The titlebar owns no sidebar state. */
  leading?: React.ReactNode
  /** Optional history controls, ordered back then forward. Omit to render neither. */
  navigation?: {
    back: WindowTitlebarAction
    forward: WindowTitlebarAction
  }
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
 * Renders a borderless header with fixed leading controls and optional history
 * actions. Place it before AppShellBody so toggling sidebars cannot move it.
 * Native dragging, window controls, theme, and navigation remain app-owned;
 * native header props are forwarded for host drag regions, refs, and events.
 * Children occupy the flexible middle region. No title or icons are invented.
 */
function WindowTitlebar({
  leading,
  navigation,
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
      {navigation
        ? [navigation.back, navigation.forward].map((action, index) => (
            <Button
              key={index}
              type="button"
              variant="ghost"
              size="icon"
              data-slot="app-shell-titlebar-navigation"
              className="h-8 w-6 text-muted-foreground active:translate-y-0"
              aria-label={action.label}
              title={action.label}
              disabled={action.disabled}
              onClick={action.onClick}
            >
              <span aria-hidden="true" className="inline-flex">
                {action.icon}
              </span>
            </Button>
          ))
        : null}
      {children ? <div className="min-w-0 flex-1">{children}</div> : null}
      {trailing ? (
        <div className="ml-auto flex shrink-0 items-center">{trailing}</div>
      ) : null}
    </header>
  )
}

export { WindowTitlebar, type WindowTitlebarAction, type WindowTitlebarProps }
