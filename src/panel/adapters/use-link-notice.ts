import * as React from "react"

import { onLinkNotOpened, type LinkNotOpened } from "../../host"
import { linkNotice, type LinkNotice } from "../application/link-notice"

/** The notice for the last link that did not open, and a way to put it away. */
export interface PanelLinkNotice {
  notice: LinkNotice | null
  dismiss: () => void
}

/**
 * The panel's side of a link that went nowhere.
 *
 * The wording is in `link-notice.ts` and tested there; the wiring is one host
 * subscription. Only the most recent click is kept — a second refused link
 * replaces the first rather than stacking, because the notice answers the
 * gesture somebody just made.
 *
 * The host's own error is logged rather than shown, the way the update failure
 * beside it does: the sentence on screen is the panel's, and the technical
 * reason belongs where a bug report can find it.
 */
export function usePanelLinkNotice(): PanelLinkNotice {
  const [link, setLink] = React.useState<LinkNotOpened | null>(null)

  React.useEffect(() => {
    let stale = false
    const subscription = onLinkNotOpened((notOpened) => {
      if (stale) return
      if (notOpened.detail)
        console.warn("[nessa] a link was not opened", notOpened.url, notOpened.detail)
      setLink(notOpened)
    })
    return () => {
      stale = true
      void subscription.then((unlisten) => unlisten())
    }
  }, [])

  const dismiss = React.useCallback(() => setLink(null), [])
  return { notice: link ? linkNotice(link) : null, dismiss }
}
