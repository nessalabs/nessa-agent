import { useState } from "react"
import { useGreetingPreference } from "../adapters/window-preferences"
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
  text,
  onTextChange,
}: {
  /** Sends the first message; without it the composer says chat is not connected. */
  onSend?: (text: string) => void
  /** Told when the person picks another model. */
  onModelChange?: (model: { provider: string; modelId: string }) => void
  /** The catalogue model the composer starts on. */
  initialModel?: { provider: string; modelId: string }
  /** What is typed in the composer and not sent, held by the caller. */
  text: string
  onTextChange: (text: string) => void
}) {
  const [page, setPage] = useState(false)
  const [greeting] = useGreetingPreference()

  return (
    <div className="desktop-home" data-page={page || undefined}>
      <div className="desktop-home-stack">
        <HeaderArt />
        <div className="desktop-home-inner">
          {greeting === "on" ? <h1 className="desktop-greeting">Working late?</h1> : null}
          <Composer
            page={page}
            onPageChange={setPage}
            onSend={onSend}
            onModelChange={onModelChange}
            initialModel={initialModel}
            text={text}
            onTextChange={onTextChange}
          />
        </div>
      </div>
    </div>
  )
}
