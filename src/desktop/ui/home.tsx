import { useState } from "react"
import { greetingFor } from "../model/greeting"
import { Composer } from "./composer"

/**
 * The first thing the window shows: a greeting, and the composer beneath it.
 * A draft that outgrows the composer turns it into a page filling the
 * workspace, headed by the same greeting (`model/page-mode.ts`). The change
 * is immediate, with no animation between the two layouts.
 */
export function Home() {
  const [greeting] = useState(() => greetingFor(new Date().getHours()))
  const [page, setPage] = useState(false)

  return (
    <div className="desktop-home" data-page={page || undefined}>
      <div className="desktop-home-inner">
        <h1 className="desktop-greeting">{greeting}</h1>
        <Composer page={page} onPageChange={setPage} />
      </div>
    </div>
  )
}
