/**
 * What one app view shows, and what a host draws for it: ADR 326's table of
 * what a host draws, for the rows only an app has (#349) — loading, live,
 * not loadable, its server gone, and a connection its CSP blocked.
 */
import type { WidgetPlace } from "../../model/widget-state"
import type { Lifecycle } from "./lifecycle"

/** How many blocked origins a view keeps to say; later ones are counted, not kept. */
export const blockedOrigins = 8

/** The view as its host draws it. */
export interface AppView {
  readonly lifecycle: Lifecycle["kind"]
  /** Why it failed, when it did. */
  readonly failed?: "load" | "server-gone"
  /** Whether the sandbox proxy's frame is on the page. */
  readonly frame: boolean
  /** Inline only: the height the app asked for (`size-changed`), clamped. */
  readonly height?: number
  /** Origins the CSP blocked a load from, in the order seen. */
  readonly blocked: readonly string[]
  /** Whether a load was blocked at all: some blocked loads have no origin to name. */
  readonly anyBlocked: boolean
  /** Whether a call to its server found the server gone. */
  readonly serverGone: boolean
}

export const firstView: AppView = {
  lifecycle: "reading",
  frame: false,
  blocked: [],
  anyBlocked: false,
  serverGone: false,
}

/** The lines a host says for an app, by why. */
export const appLines = {
  load: "This app couldn't be loaded",
  serverGone: "This app's server has stopped",
  blocked: "Blocked a connection this app didn't declare",
} as const

/** What a host draws for an app view in `place`. */
export interface AppDraws {
  /** The frame, when it is on the page — hidden beneath the placeholder until the app is live. */
  readonly frame: "hidden" | "shown" | "none"
  /** The quiet placeholder with the plugin's name, while the app loads. */
  readonly waiting: boolean
  /** One line in the app's stead, with close where the place has one. */
  readonly line?: { readonly text: string; readonly closes: boolean }
  /** Notices above a live app. */
  readonly notices: readonly string[]
}

export function appDraws(place: WidgetPlace, view: AppView): AppDraws {
  const closes = place !== "inline"
  if (view.lifecycle === "failed")
    return {
      frame: "none",
      waiting: false,
      line: {
        text: view.failed === "server-gone" ? appLines.serverGone : appLines.load,
        closes,
      },
      notices: [],
    }
  const live = view.lifecycle === "live" || view.lifecycle === "ending"
  const notices = [
    ...(view.serverGone ? [appLines.serverGone] : []),
    ...(view.anyBlocked
      ? [
          view.blocked.length > 0
            ? `${appLines.blocked}: ${view.blocked.join(", ")}`
            : appLines.blocked,
        ]
      : []),
  ]
  return {
    frame: !view.frame ? "none" : live ? "shown" : "hidden",
    waiting: !live && view.lifecycle !== "gone",
    notices,
  }
}
