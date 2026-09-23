import type { Stage } from "./stage.js"

/** Context supplied when resolving a gateway publication. */
export type GatewayEndpointContext = {
  /** Stage whose isolated local namespace is being addressed. */
  stage: Stage
}

/**
 * Resolves a server-published local endpoint before credentials are loaded.
 *
 * `undefined` means no readable publication exists, so the client's existing
 * stage fallback policy applies. A present publication that is malformed or
 * does not correlate with unauthenticated health must reject instead; treating
 * it as absent could send credentials to the fallback socket.
 */
export interface GatewayEndpointSource {
  load(context: GatewayEndpointContext): Promise<string | undefined>
}

/** A present endpoint publication cannot safely be used. */
export class NessaEndpointDiscoveryError extends Error {
  constructor(message = "The published gateway endpoint could not be verified") {
    super(message)
    this.name = "NessaEndpointDiscoveryError"
  }
}
