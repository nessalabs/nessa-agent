import type { Stage } from "./stage.js"

/** Trusted host storage. A source loads evidence; it cannot grant gateway permissions. */
export interface CredentialSource {
  load(context: { clientId: string; stage: Stage; url: string }): Promise<string>
}

/** No credential could be loaded from the selected host storage. */
export class NessaCredentialUnavailableError extends Error {
  constructor(
    message = "No local surface credential is available. Provision this surface before connecting.",
  ) {
    super(message)
    this.name = "NessaCredentialUnavailableError"
  }
}
