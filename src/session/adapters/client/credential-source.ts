import {
  hasNativeHost,
  loadAssignedGatewayEndpoint,
  loadAssignedSurfaceCredential,
} from "../../../host"
import {
  isLoopbackWebSocketUrl,
  NessaCredentialUnavailableError,
  type CredentialSource,
  type GatewayEndpointSource,
} from "@nessa/client"

/** The native host verifies the server publication before returning its URL. */
export function nativeGatewayEndpointSource(): GatewayEndpointSource | undefined {
  if (!hasNativeHost()) return undefined
  return { load: ({ stage }) => loadAssignedGatewayEndpoint(stage) }
}

/** The native host exposes only the bundled chat credential, never arbitrary paths. */
export function nativeCredentialSource(): CredentialSource | undefined {
  if (!hasNativeHost()) return undefined
  return {
    async load({ stage, url, clientId }) {
      if (clientId !== "nessa-panel" || !isLoopbackWebSocketUrl(url))
        throw new NessaCredentialUnavailableError(
          "The native chat credential is only available to the local chat surface",
        )
      return loadAssignedSurfaceCredential(stage, url)
    },
  }
}
