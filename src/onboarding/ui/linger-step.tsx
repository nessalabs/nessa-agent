import * as React from "react"
import { Button } from "@nessa-ui/react/button"

import type { LingerShown, LingerView } from "../model/linger"
import { PANE_BUTTON, SETUP_HEADING_ID, SetupPanel, SetupStage } from "./setup-frame"

/** Sentences for one host tag. `enabled` is the only claim of logged-out operation. */
function copy(shown: LingerShown): { heading: string; detail: string } {
  switch (shown) {
    case "offer":
      return {
        heading: "Keep Nessa running when you log out?",
        detail:
          "The gateway stops when you log out unless this account lingers. Turning that on asks for an administrator.",
      }
    case "enabled":
      return {
        heading: "Nessa keeps running when you log out",
        detail: "This account lingers, so the gateway stays running after you log out.",
      }
    case "declined":
      return {
        heading: "Nessa runs while you are signed in",
        detail: "It stops when you log out.",
      }
    case "refused":
      return {
        heading: "Nessa runs while you are signed in",
        detail: "Staying on after logout was not turned on.",
      }
    case "unsupported":
      return {
        heading: "Nessa cannot stay running after you log out",
        detail: "This system has no login service to ask.",
      }
    case "unconfirmed":
    case "not-applicable":
      return {
        heading: "Nessa could not confirm that",
        detail: "Login did not say whether this account lingers.",
      }
    case "waiting":
      return {
        heading: "Waiting for permission",
        detail:
          "An administrator has to allow Nessa to keep running after you log out.",
      }
  }
}

/**
 * The linger step. What it says is the host's `shown` tag. A prompt still open
 * is waiting, and waiting offers neither another enable nor a way on that would
 * close the window under the prompt.
 */
export const LingerStep = React.forwardRef<
  HTMLDivElement,
  {
    view: LingerView | undefined
    pending: boolean
    onAccept: () => void
    onDecline: () => void
    onFinish: () => void
  }
>(function LingerStep({ view, pending, onAccept, onDecline, onFinish }, ref) {
  const shown: LingerShown =
    pending || view?.shown === "waiting" ? "waiting" : (view?.shown ?? "unconfirmed")
  const claim = view?.shown === "enabled" && !pending ? "yes" : "no"
  const { heading, detail } = copy(shown)
  return (
    <SetupStage>
      <SetupPanel ref={ref}>
        <div
          className="flex flex-col gap-3 text-center"
          data-linger={shown}
          data-linger-claim={claim}
        >
          <h1 id={SETUP_HEADING_ID} className="nessa-text-6 font-semibold text-foreground">
            {heading}
          </h1>
          <p role="status" aria-live="polite" className="nessa-text-3 text-muted-foreground">
            {detail}
          </p>
          {view?.audit === "failed" && !pending ? (
            <p className="nessa-text-3 text-muted-foreground">
              Nessa could not record that decision.
            </p>
          ) : null}
        </div>
        {shown === "offer" ? (
          <>
            <Button size="lg" className={PANE_BUTTON} onClick={onAccept}>
              Keep it running
            </Button>
            <Button size="lg" className={PANE_BUTTON} onClick={onDecline}>
              Only while I’m signed in
            </Button>
          </>
        ) : null}
        {shown === "refused" ? (
          <>
            <Button size="lg" className={PANE_BUTTON} onClick={onAccept}>
              Try again
            </Button>
            <Button size="lg" className={PANE_BUTTON} onClick={onFinish}>
              Start using Nessa
            </Button>
          </>
        ) : null}
        {shown === "enabled" ||
        shown === "declined" ||
        shown === "unsupported" ||
        shown === "unconfirmed" ||
        shown === "not-applicable" ? (
          <Button size="lg" className={PANE_BUTTON} onClick={onFinish}>
            Start using Nessa
          </Button>
        ) : null}
      </SetupPanel>
    </SetupStage>
  )
})
