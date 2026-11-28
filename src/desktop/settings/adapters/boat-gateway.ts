import type { BoatApi, BoatStatus } from "@nessa/client"
/** Narrow settings dependency. Boat is connected only by Sign in with Boat:
 * the gateway holds the sign-in, and the window never sees a token. */
export interface BoatGateway {
  read(): Promise<BoatStatus>
  /** Begin Sign in with Boat on the gateway; its status names the page to open. */
  signIn(): Promise<BoatStatus>
  cancelSignIn(): Promise<BoatStatus>
  disconnect(): Promise<BoatStatus>
  /** Open Boat's sign-in page in the person's own browser. */
  open(url: string): void
  /** How often a sign-in in progress is read again. */
  readonly pollMs: number
}
export function boatGateway(
  connected: () => Promise<{ readonly boat: BoatApi }>,
  open: (url: string) => void,
  pollMs = 2000,
): BoatGateway {
  return {
    read: async () => (await connected()).boat.status(),
    signIn: async () => (await connected()).boat.signIn(),
    cancelSignIn: async () => (await connected()).boat.cancelSignIn(),
    disconnect: async () => (await connected()).boat.disconnect(),
    open,
    pollMs,
  }
}

/**
 * Boat's page in the person's browser. In the app a plain link's navigation
 * is handed to the system browser and the window stays where it is
 * (`src-tauri/src/links.rs`), while `window.open` is not seen there; in a
 * browser tab a new tab is the browser.
 */
export function openInBrowser(nativeHost: boolean): (url: string) => void {
  return (url) => {
    if (nativeHost) {
      const link = document.createElement("a")
      link.href = url
      link.rel = "noreferrer"
      link.click()
    } else window.open(url, "_blank", "noopener,noreferrer")
  }
}
