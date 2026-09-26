import * as React from "react"
import { nativeGatewayStartup } from "../adapters/gateway-startup"
import {
  createGatewayStartupMonitor,
  type GatewayStartupStatus,
} from "../application/gateway-startup"
import type { GatewayStartupSource } from "../application/ports"

/**
 * The host's startup projection for a surface that is not setup (ADR 221).
 * `undefined` until the host has answered, so a surface says nothing about
 * startup before it knows something.
 */
export function useGatewayStartup(source: GatewayStartupSource = nativeGatewayStartup): {
  status: GatewayStartupStatus | undefined
  retry: () => void
} {
  const [status, setStatus] = React.useState<GatewayStartupStatus>()
  const monitor = React.useMemo(
    () => createGatewayStartupMonitor(source, setStatus),
    [source],
  )
  React.useEffect(() => {
    monitor.start()
    return () => monitor.stop()
  }, [monitor])
  const retry = React.useCallback(() => void monitor.retry(), [monitor])
  return { status, retry }
}
