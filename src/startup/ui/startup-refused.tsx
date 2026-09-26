import { Button } from "@nessa-ui/react/button"
import { COULD_NOT_START } from "../application/copy"
import { StartupDetails } from "./startup-details"

/**
 * What the panel shows when the host could not put itself together (ADR 221).
 * One plain sentence and one thing to do; the reason is folded behind Details
 * for whoever helps the person. Nothing else is mounted: no session, no
 * conversation, nothing that would ask the host for what it could not build.
 */
export function StartupRefused({
  details,
  onTryAgain,
  onQuit,
}: {
  details: string
  onTryAgain: () => void
  onQuit: () => void
}) {
  return (
    <main className="flex size-full items-center justify-center p-4">
      <div
        role="alert"
        className="flex w-full max-w-sm flex-col gap-3 rounded-2xl border border-border bg-background p-6 text-foreground shadow-2xl"
      >
        <h1 className="nessa-text-5 font-semibold">{COULD_NOT_START}</h1>
        <p className="nessa-text-2 text-muted-foreground">
          Try again. If it keeps happening, copy the details and send them to us.
        </p>
        <div className="flex gap-2">
          <Button type="button" className="flex-1 rounded-full" onClick={onTryAgain}>
            Try again
          </Button>
          <Button
            type="button"
            variant="outline"
            className="flex-1 rounded-full"
            onClick={onQuit}
          >
            Quit
          </Button>
        </div>
        <StartupDetails details={details} />
      </div>
    </main>
  )
}
