/**
 * What the page says of itself that an app is given beside the widget host
 * context (`model/host-context.ts`, `PageContext`): the design system's
 * tokens as MCP Apps' standard style variables (*Theming*), the person's time
 * zone, and whether this is the desktop app or a browser.
 *
 * The values are the tokens as the document resolves them now, so they
 * follow the theme: the theme is part of the widget host context, and a
 * change of it reads them again.
 */
import type { PageContext } from "../../model/host-context"

/**
 * MCP Apps' variable for each design-system token it has one for. Only these
 * are given; an app keeps its own defaults for the rest, as the spec asks of
 * it.
 */
const tokenFor: Readonly<Record<string, string>> = {
  "--color-background-primary": "--background",
  "--color-background-secondary": "--muted",
  "--color-background-tertiary": "--accent",
  "--color-background-inverse": "--foreground",
  "--color-background-danger": "--destructive",
  "--color-text-primary": "--foreground",
  "--color-text-secondary": "--muted-foreground",
  "--color-text-inverse": "--background",
  "--color-text-danger": "--destructive",
  "--color-border-primary": "--border",
  "--color-border-secondary": "--input",
  "--color-ring-primary": "--ring",
  "--font-sans": "--font-sans",
  "--font-mono": "--font-mono",
  "--border-radius-xs": "--radius-xs",
  "--border-radius-sm": "--radius-sm",
  "--border-radius-md": "--radius-md",
  "--border-radius-lg": "--radius-lg",
  "--border-radius-xl": "--radius-xl",
}

/** The page's context, read from `document` now. */
export function readPageContext(
  document: Document,
  platform: PageContext["platform"],
): PageContext {
  const view = document.defaultView
  const computed = view ? view.getComputedStyle(document.documentElement) : undefined
  const styles: Record<string, string> = {}
  for (const [variable, token] of Object.entries(tokenFor)) {
    const value = computed?.getPropertyValue(token).trim()
    if (value) styles[variable] = value
  }
  return {
    styles,
    timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    platform,
  }
}
