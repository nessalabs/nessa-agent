import { gatewayStartup, onGatewayStartup, retryGatewayStartup } from "../../host"
import type { GatewayStartupSource } from "../application/ports"

/** Maps the desktop window seam into setup's gateway startup port. */
export const nativeGatewayStartup: GatewayStartupSource = {
  snapshot: gatewayStartup,
  subscribe: onGatewayStartup,
  retry: retryGatewayStartup,
}
