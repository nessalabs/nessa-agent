import { hasNativeHost, loadAssignedSurfaceCredential } from "../../../host"
import {
  isLoopbackWebSocketUrl,
  NessaCredentialUnavailableError,
  type CredentialSource,
} from "@nessa/client"

/** The native host exposes only the bundled chat credential, never arbitrary paths. */
export function nativeCredentialSource(): CredentialSource | undefined {
  if (!hasNativeHost()) return undefined
  return {
    async load({ stage, url, clientId }) {
      if (clientId !== "nessa-panel" || !isLoopbackWebSocketUrl(url))
        throw new NessaCredentialUnavailableError(
          "The native chat credential is only available to the local chat surface",
        )
      return loadAssignedSurfaceCredential(stage)
    },
  }
}
