/**
 * What a plugin says of one of its widgets, where a widget can be drawn, and
 * what a view may read of the place it is drawn in (ADR 326).
 */

/**
 * A plugin's answer for one widget (`useWidget`), reactively: `ready` with
 * its title and, when it belongs to a session, that session's id (`origin`);
 * `unread`, not read yet (a source that reads over the wire); `missing`,
 * gone; `off`, the plugin's preview is turned off in Settings; `unshowable`,
 * held but not drawable (an experiment that failed validation).
 */
export type WidgetState =
  | { readonly kind: "ready"; readonly title: string; readonly origin?: string }
  | { readonly kind: "unread" }
  | { readonly kind: "missing" }
  | { readonly kind: "off" }
  | { readonly kind: "unshowable" }

/**
 * What the window knows of a widget: no plugin registered under its
 * reference's plugin id, or the registered plugin's name — what the window
 * calls it while it is not read — and its answer.
 */
export type WidgetAnswer =
  | { readonly registered: false }
  | { readonly registered: true; readonly name: string; readonly state: WidgetState }

/**
 * Where a widget is drawn: a card in a message, a pane of its own, or the
 * window — the content region's third view, over the panes.
 */
export type WidgetPlace = "inline" | "pane" | "window"

/** The places a widget is opened in; `inline` is the transcript's, never asked for. */
export type OpenPlace = Exclude<WidgetPlace, "inline">

/** The places a plugin offers a view for, beside the pane every plugin has. */
export interface OfferedPlaces {
  readonly inline: boolean
  readonly window: boolean
}

/**
 * What a view may read of where it is drawn: shaped after MCP Apps'
 * `hostContext`, so an app's bridge passes it on as it is (ADR 344). Read
 * only; a change draws the view again.
 */
export interface HostContext {
  readonly theme: "light" | "dark"
  /** The window's language, as the browser names it: `en-US`. */
  readonly locale: string
  /** The place's content box in whole CSS pixels; `null` until it is laid out. */
  readonly size: { readonly width: number; readonly height: number } | null
  /** What of the place the window's own chrome covers, in CSS pixels. */
  readonly safeArea: {
    readonly top: number
    readonly right: number
    readonly bottom: number
    readonly left: number
  }
}
