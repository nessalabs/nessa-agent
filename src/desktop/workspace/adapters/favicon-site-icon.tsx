import { Favicon } from "@nessa-ui/react/favicon"
import type { SiteIcon } from "../ui/transcript/site-icon"

/** The window's `SiteIcon`: the kit's lazy favicon, which sends no referrer. */
export const faviconSiteIcon: SiteIcon = ({ href, fallback }) => (
  <Favicon href={href} fallback={fallback} className="workspace-step-favicon" />
)
