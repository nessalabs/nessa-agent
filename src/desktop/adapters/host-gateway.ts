/**
 * The desktop app's connection to the local gateway (#419): the panel's own
 * session (`connectDevSession`) over what the host hands a bundled surface —
 * the gateway's endpoint and its surface credential
 * (`nativeGatewayEndpointSource`, `nativeCredentialSource`), the host
 * admitting this window by its label (`bundled_window` in `src-tauri`) — as
 * the window it is (`surfaceKind: "desktop"`).
 *
 * The credential is the bundled surface's, which the session's client id
 * names (`connectDevSession`'s `nessa-panel`), not the window; the window is
 * told apart by its surface kind and instance. The token goes from the host
 * to the client's handshake and nowhere else: not the URL, not storage, not
 * a log.
 */
import type { CredentialSource, GatewayEndpointSource, NessaClient } from "@nessa/client"
import type { Environment } from "../../env/environment"
import {
  connectDevSession,
  nativeCredentialSource,
  nativeGatewayEndpointSource,
} from "../../session"

export function hostGateway(
  environment: Pick<Environment, "stage" | "gatewayBaseUrlOverride">,
  dependencies: {
    /** The client's own connect; tests pass theirs. */
    readonly connect?: typeof NessaClient.connect
    readonly credentialSource?: CredentialSource
    readonly endpointSource?: GatewayEndpointSource
  } = {},
): () => Promise<NessaClient> {
  const credentialSource = dependencies.credentialSource ?? nativeCredentialSource()
  const endpointSource = dependencies.endpointSource ?? nativeGatewayEndpointSource()
  return () =>
    connectDevSession({
      connect: dependencies.connect,
      stage: environment.stage,
      gatewayBaseUrl: environment.gatewayBaseUrlOverride,
      surfaceKind: "desktop",
      credentialSource,
      endpointSource,
    }).then((session) => session.client)
}
