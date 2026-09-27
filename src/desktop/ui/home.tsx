import { useState } from "react"
import { Composer } from "./composer"
import { HeaderArt } from "./header-art"

/**
 * The first thing the window shows: a rainy night scene across the top, or a
 * picture the person chose in its place (`ui/header-art.tsx`), then
 * "Working late?", the scene's own caption, whatever the hour, and the
 * composer beneath it.
 * A draft that outgrows the composer turns it into a page filling the
 * workspace, headed by the same greeting (`model/page-mode.ts`). The change
 * is immediate, with no animation between the two layouts.
 */
export function Home() {
  const [page, setPage] = useState(false)

  return (
    <div className="desktop-home" data-page={page || undefined}>
      <div className="desktop-home-stack">
        <HeaderArt />
        <div className="desktop-home-inner">
          <h1 className="desktop-greeting">Working late?</h1>
          <Composer page={page} onPageChange={setPage} />
        </div>
      </div>
    </div>
  )
}
