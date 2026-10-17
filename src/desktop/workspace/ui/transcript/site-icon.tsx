import { createContext, useContext, type ReactNode } from "react"

/**
 * Draws a web page's own icon, with `fallback` standing in until it loads
 * and wherever the site has none. Asking a site for its icon is a request
 * leaving the process, so composition supplies the renderer that does it;
 * tests supply their own.
 */
export type SiteIcon = (props: { href: string; fallback: ReactNode }) => ReactNode

const SiteIconContext = createContext<SiteIcon | null>(null)

export function SiteIconProvider({
  render,
  children,
}: {
  render: SiteIcon
  children: ReactNode
}) {
  return <SiteIconContext.Provider value={render}>{children}</SiteIconContext.Provider>
}

/** The composed renderer, or none: then only the fallback is drawn and nothing is fetched. */
export function useSiteIcon(): SiteIcon | null {
  return useContext(SiteIconContext)
}
