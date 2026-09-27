import { useState } from "react"
import { Composer } from "./composer"
import { HeaderArt } from "./header-art"

/**
 * The first thing the window shows: a rainy night scene across the top, or a
 * picture the person chose in its place (`ui/header-art.tsx`), then
 * "Working late?", the scene's own caption, whatever the hour, and the
 * composer beneath it. A new session's pane shows the same home, its
 * composer wired to that session through the props below.
 * A draft that outgrows the composer turns it into a page filling the
 * workspace, headed by the same greeting (`model/page-mode.ts`). The change
 * is immediate, with no animation between the two layouts.
 */
export function Home({
  onSend,
  onModelChange,
  initialModel,
}: {
  /** Sends the first message; without it the composer says chat is not connected. */
  onSend?: (text: string) => void
  /** Told when the person picks another model. */
  onModelChange?: (model: { provider: string; modelId: string }) => void
  /** The catalogue model the composer starts on. */
  initialModel?: { provider: string; modelId: string }
} = {}) {
  const [page, setPage] = useState(false)

  return (
    <div className="desktop-home" data-page={page || undefined}>
      <div className="desktop-home-stack">
        <HeaderArt />
        <div className="desktop-home-inner">
          <h1 className="desktop-greeting">Working late?</h1>
          <Composer
            page={page}
            onPageChange={setPage}
            onSend={onSend}
            onModelChange={onModelChange}
            initialModel={initialModel}
          />
        </div>
      </div>
    </div>
  )
}
