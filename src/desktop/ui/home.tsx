import { useState } from "react"
import { useGreetingPreference } from "../adapters/window-preferences"
import { Composer } from "./composer"
import { HeaderArt } from "./header-art"
import { greetingAt } from "../model/greeting"
import { useNow } from "../workspace/adapters/dom/clock"

/**
 * The first thing the window shows: a rainy night scene across the top, or a
 * picture the person chose in its place (`ui/header-art.tsx`), then a
 * greeting for the hour of the person's day (`model/greeting.ts`, read from
 * the window's clock each minute) — "Working late?" only when it is — and the
 * composer beneath it. A new session's pane shows the same home, its
 * composer wired to that session through the props below.
 * A draft that outgrows the composer turns it into a page filling the
 * workspace, headed by the same greeting (`model/page-mode.ts`). The change
 * is immediate, with no animation between the two layouts.
 *
 * The composer sits in a box of its own, which a pane names with
 * `composerClassName` so a new session's home in a small pane can dock it
 * as a conversation does (`workspace/ui/panes/conversation.css`).
 */
export function Home({
  onSend,
  model,
  onModelChange,
  text,
  onTextChange,
  composerClassName,
}: {
  /** Sends the first message; without it the composer says chat is not connected. */
  onSend?: (text: string) => void
  /** The model the first message is sent with, held by the caller, as the text is. */
  model: { provider: string; modelId: string } | undefined
  /** Told when the person picks another model. */
  onModelChange: (model: { provider: string; modelId: string }) => void
  /** What is typed in the composer and not sent, held by the caller. */
  text: string
  onTextChange: (text: string) => void
  /** A class for the box around the composer. */
  composerClassName?: string
}) {
  const [page, setPage] = useState(false)
  const [greeting] = useGreetingPreference()
  const now = useNow(60_000)

  return (
    <div className="desktop-home" data-page={page || undefined}>
      <div className="desktop-home-stack">
        <HeaderArt />
        <div className="desktop-home-inner">
          {greeting === "on" ? (
            <h1 className="desktop-greeting">{greetingAt(new Date(now).getHours())}</h1>
          ) : null}
          <div
            className={
              composerClassName
                ? `desktop-home-composer ${composerClassName}`
                : "desktop-home-composer"
            }
          >
            <Composer
              page={page}
              onPageChange={setPage}
              onSend={onSend}
              model={model}
              onModelChange={onModelChange}
              text={text}
              onTextChange={onTextChange}
            />
          </div>
        </div>
      </div>
    </div>
  )
}
