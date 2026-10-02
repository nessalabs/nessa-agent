/**
 * The host context an app is given (MCP Apps, *Host Context in
 * `McpUiInitializeResult`*): the widget host's own context (ADR 326's
 * `HostContext`, already shaped after it) translated field for field, with
 * what only an app is told — its display mode, the modes offered, its
 * container's dimensions, the page's style variables, time zone and
 * platform, and the tool call it was made for.
 *
 * Built whole each time; `changedContext` then says which fields differ,
 * which is all `ui/notifications/host-context-changed` carries.
 */
import type { HostContext, WidgetPlace } from "../../model/widget-state"
import type { JsonObject } from "./json-rpc"
import { modeOf, offeredModes } from "./places"

/** What the page says of itself that the widget host context does not. */
export interface PageContext {
  /** MCP Apps' standard style variables, by name (`--color-text-primary`), as the page's tokens set them. */
  readonly styles: Readonly<Record<string, string>>
  /** The person's time zone, as IANA names it. */
  readonly timeZone: string
  readonly platform: "web" | "desktop"
}

/** The tallest an inline card grows to follow its app; beyond it the app scrolls inside. */
export const inlineMaxHeight = 600

/** The context for a view drawn in `place`; `tool` is the call's tool, as the server described it. */
export function appHostContext(
  place: WidgetPlace,
  context: HostContext,
  page: PageContext,
  tool: JsonObject | undefined,
): JsonObject {
  const { size } = context
  // Inline, the width is the card's and the height the app's, up to a
  // limit; in a pane or the window, the place's box is the app's.
  const containerDimensions: JsonObject =
    place === "inline"
      ? size
        ? { width: size.width, maxHeight: inlineMaxHeight }
        : { maxHeight: inlineMaxHeight }
      : size
        ? { width: size.width, height: size.height }
        : {}
  return {
    ...(tool ? { toolInfo: { tool } } : {}),
    theme: context.theme,
    styles: { variables: { ...page.styles } },
    displayMode: modeOf(place),
    availableDisplayModes: [...offeredModes],
    containerDimensions,
    locale: context.locale,
    timeZone: page.timeZone,
    platform: page.platform,
    safeAreaInsets: { ...context.safeArea },
  }
}

/** The fields of `after` that differ from `before`, or `undefined` when none do. */
export function changedContext(
  before: JsonObject,
  after: JsonObject,
): JsonObject | undefined {
  const changed: Record<string, JsonObject[string]> = {}
  for (const [key, value] of Object.entries(after)) {
    const was = Object.hasOwn(before, key) ? JSON.stringify(before[key]) : undefined
    if (was !== JSON.stringify(value)) changed[key] = value
  }
  return Object.keys(changed).length > 0 ? changed : undefined
}
