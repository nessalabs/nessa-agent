import type { ReactNode } from "react"
import { statusLabels } from "../../model/session-groups"
import { useSettledWorking } from "./use-settled-working"

/**
 * Words that name something an agent is working on, with a highlight
 * sweeping across them (`chrome.css` › Words at work): the window's one sign
 * of work, in place of a turning arc beside the words. Nothing at work, they
 * are the plain words. Work shows only once it lasts (`useSettledWorking`).
 *
 * `spoken` adds "Running" for a reader, where nothing else around the words
 * says it; a row whose name already says it leaves it off.
 */
export function WorkingWords({
  working,
  spoken = false,
  children,
}: {
  working: boolean
  spoken?: boolean
  children: ReactNode
}) {
  const shown = useSettledWorking(working)
  if (!shown) return <>{children}</>
  return (
    <>
      <span className="workspace-shimmer" data-working="">
        {children}
      </span>
      {spoken ? <span className="workspace-spoken"> {statusLabels.running}</span> : null}
    </>
  )
}
