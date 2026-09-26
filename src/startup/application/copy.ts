import type { GatewayStartupStatus } from "./gateway-startup"

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
      return "Nessa couldn’t start."
    case "unavailable":
      return "Nessa couldn’t check whether it started."
    default:
      return undefined
  }
}

/** The technical account behind "Details", when there is one. */
export function startupDetails(status: GatewayStartupStatus): string | undefined {
  return status.state === "failed" || status.state === "unavailable"
    ? status.message
    : undefined
}
