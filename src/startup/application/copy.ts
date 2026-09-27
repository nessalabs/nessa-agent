import type { GatewayStartupStatus } from "./gateway-startup"

/** What a person reads when Nessa could not start, wherever it is said. */
export const COULD_NOT_START = "Nessa couldn’t start"

/** The line under it, when trying again is the thing to do. */
export const TRY_AGAIN_HINT = "Trying again usually fixes this."

/**
 * The one sentence a person reads about startup (ADR 221), shared by setup and
 * the panel so they cannot word it differently. The host's own message is
 * technical and stays behind "Details"; nothing here reads it.
 */
export function startupSentence(status: GatewayStartupStatus): string | undefined {
  switch (status.state) {
    case "starting":
      // The step arrives from the host. A step this page does not know is
      // still a start in progress, so it reads as the first one.
      switch (status.step) {
        case "replacing":
          return "Finishing the last update…"
        case "launching":
          return "Starting…"
        default:
          return "Getting ready…"
      }
    case "failed":
      return COULD_NOT_START
    case "unavailable":
      return "Nessa couldn’t check whether it started."
    default:
      return undefined
  }
}
