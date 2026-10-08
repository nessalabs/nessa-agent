/**
 * The desktop app's connection to the local gateway (#419): the panel's own
 * session (`connectDevSession`) over what the host serves this window as a
 * reader of the gateway — its endpoint and the panel's surface credential
 * (`nativeGatewayEndpointSource`, `nativeCredentialSource`), once the
 * host's startup of the gateway is ready (`GatewayReader` in `src-tauri`).
 *
 * The credential is the panel's, which the session's client id
 * names (`connectDevSession`'s `nessa-panel`), not the window. Both
 * authenticate as client `nessa-panel` and are answered as principal
 * `surface:nessa-panel`. This window's handshake also sends
 * `surface: { kind: "desktop", instance }` (`surfaceKind` below;
 * `SessionAuthenticateParams.surface`), which the gateway records, and
 * `verification/desktop/scripts/gateway-window.mjs` asserts that kind on
 * every handshake this window makes (W4′). This module never holds the
 * token: it hands the client the credential source, which the client asks at
 * its handshake, and passes no URL or `auth` of its own
 * (`host-gateway.test.ts`).
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
