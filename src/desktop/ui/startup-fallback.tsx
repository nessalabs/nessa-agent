import { useEffect } from "react"
import { quitNessa, restartNessa } from "../../host"
import { StartupScreen } from "../../startup/ui/startup-screen"
import { useWorkspaceSelector } from "../workspace/adapters/store/hooks"
import { selectFailure, selectFailureStages } from "../workspace/adapters/store/selectors"
import { readFailureCopy } from "../workspace/ui/failure-copy"
import { startupFailureCode } from "../workspace/ui/startup-failure"

/**
 * The desktop window's startup screen. The workspace stays mounted under it,
 * so a server that starts later clears the screen on its own. Restart and
 * Quit are the actions; the sentence in the log is the detailed cause.
 */
export function StartupFallback() {
  const failure = useWorkspaceSelector(selectFailure)
  const stages = useWorkspaceSelector(selectFailureStages)
  const code = startupFailureCode(failure)
  useEffect(() => {
    if (!failure || !code) return
    console.error(`[nessa] ${readFailureCopy(failure, "index", stages)}`)
  }, [failure, stages, code])
  if (!code) return null
  return (
    <StartupScreen
      code={code}
      onRestart={() => void restartNessa()}
      onQuit={() => void quitNessa()}
    />
  )
}
