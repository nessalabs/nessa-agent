import { useState } from "react"
import { ArrowUp } from "lucide-react"
import { greetingFor } from "../model/greeting"

/**
 * The first thing the window shows. The composer takes text but cannot send:
 * this shell has no conversation wiring yet, and says so rather than pretending.
 */
export function Home() {
  const [greeting] = useState(() => greetingFor(new Date().getHours()))
  const [draft, setDraft] = useState("")

  return (
    <div className="desktop-home">
      <div className="desktop-home-inner">
        <h1 className="desktop-greeting">{greeting}</h1>
        <p className="desktop-subtitle">What would you like to work on?</p>
        <form className="desktop-composer" onSubmit={(event) => event.preventDefault()}>
          <textarea
            aria-label="Message"
            placeholder="Ask nessa anything…"
            rows={3}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
          <div className="desktop-composer-row">
            <span className="desktop-composer-hint">
              Chat isn’t connected in this preview
            </span>
            <button
              type="submit"
              className="desktop-send"
              aria-label="Send"
              title="Chat isn’t connected yet"
              disabled
            >
              <ArrowUp aria-hidden="true" />
            </button>
          </div>
        </form>
      </div>
    </div>
  )
}
