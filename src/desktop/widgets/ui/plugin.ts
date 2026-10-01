/**
 * The plugin contract (ADR 326): what a plugin gives the window, and what the
 * window gives each view it draws. Two kinds behind the same hosts, places
 * and states — `native`, in-process React the window trusts, and `app`, an
 * MCP App drawn in a sandboxed frame (ADR 344) — told apart by `kind` alone.
 *
 * The host's callbacks are shaped after MCP Apps' `ui/*` bridge, so the `app`
 * kind is a translation rather than a second contract: `open(place)` is
 * `ui/request-display-mode` (`inline`, `fullscreen` → `pane`), the host
 * context is `hostContext`. Sending a message and updating the model's
 * context (`ui/message`, `ui/update-model-context`) belong to both kinds and
 * arrive with the `app` host (#349); nothing here offers them yet.
 */
import type { ComponentType } from "react"
import type { WidgetRegistry } from "../application/registry"
import type { WidgetRef } from "../model/widget-ref"
import type {
  HostContext,
  OpenPlace,
  WidgetPlace,
  WidgetState,
} from "../model/widget-state"

/** What the host does for a view: the only way a view reaches the window. */
export interface WidgetHost {
  /** Shows this widget in another place (`ui/request-display-mode`); where it is already, nothing. */
  open(place: OpenPlace): void
  /** Closes it where it is: its pane, or the window back to the panes. A card in a message has none. */
  close(): void
  /**
   * Opens another plugin's widget. Its pane goes beside the pane showing
   * this widget's `origin` — the host holds it, as only the plugin's own
   * hook knows it — or in the focused pane's place.
   */
  openWidget(widget: WidgetRef, place: OpenPlace): void
  /**
   * Registers a step back for Escape while the view has somewhere to step
   * back to (a detail it opened); returns its removal. Escape reaching the
   * host runs the one registered last before the host's own.
   */
  onEscape(stepBack: () => void): () => void
}

/** What a view is given: the widget's id, where it is drawn, the host, and the place's context. */
export interface WidgetViewProps {
  readonly id: string
  readonly place: WidgetPlace
  readonly host: WidgetHost
  readonly context: HostContext
}

/**
 * What a plugin draws in a session pane's header, for that session: given
 * its id and `openWidget` alone — it has no widget of its own to open, close
 * or step back in. What it opens goes beside that session's pane.
 */
export interface SessionAccessoryProps {
  readonly sessionId: string
  readonly openWidget: (widget: WidgetRef, place: OpenPlace) => void
}

/** A plugin drawn by the window itself, registered once, in composition. */
export interface NativeWidgetPlugin {
  readonly kind: "native"
  readonly id: string
  /** What the window calls one of its widgets while it is not read: "Experiment". */
  readonly name: string
  /** The plugin's hook: what it says of widget `id`, reactively. */
  readonly useWidget: (id: string) => WidgetState
  /** A view per place: every plugin has a pane; a card in a message and the window are its choice. */
  readonly views: {
    readonly pane: ComponentType<WidgetViewProps>
    readonly inline?: ComponentType<WidgetViewProps>
    readonly window?: ComponentType<WidgetViewProps>
  }
  readonly SessionAccessory?: ComponentType<SessionAccessoryProps>
}

/**
 * An MCP App, registered while the window runs (ADR 344). Its renderer, the
 * sandboxed frame and the bridge, is #349; until it lands, a host draws one
 * of its widgets as one it cannot show.
 */
export interface AppWidgetPlugin {
  readonly kind: "app"
  readonly id: string
  readonly name: string
}

export type WidgetPlugin = NativeWidgetPlugin | AppWidgetPlugin

/** The window's registry, over its plugins. */
export type DesktopWidgetRegistry = WidgetRegistry<WidgetPlugin>
