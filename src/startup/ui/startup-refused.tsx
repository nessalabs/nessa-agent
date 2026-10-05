import { useEffect } from "react"
import { startupCode } from "../../host/startup-refusals"
import { StartupScreen } from "./startup-screen"

/**
 * What the panel shows when the host could not put itself together.
 * The same calm screen as every other startup failure: the shared line, the
 * code, Restart and Quit. The technical reason is logged (ADR 221); the
 * screen does not show it.
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
  useEffect(() => {
    if (details) console.error(`[nessa] ${details}`)
  }, [details])
  return (
    <StartupScreen code={startupCode("host")} onRestart={onTryAgain} onQuit={onQuit} />
  )
}
