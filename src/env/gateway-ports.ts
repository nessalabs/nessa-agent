import table from "../../protocol/defaults/gateway-ports.json"

export type Stage = "dev" | "ci" | "alpha" | "prod"

const stages: Readonly<Record<string, number>> = table.stages

/**
 * Loopback port the gateway listens on for `stage` when `NESSA_PORT` says
 * nothing.
 *
 * `protocol/defaults/gateway-ports.json` is the one table: `nessa-server` and
 * the desktop host compile the same bytes in, and the Vite proxy imports this
 * function. 7420 is the product port an installed Nessa holds through a
 * background service, so the dev stage listens next to it instead of fighting
 * it for the socket.
 */
export function gatewayPort(stage: Stage): number {
  const port = stages[stage]
  if (port === undefined) throw new Error(`No gateway port for stage ${stage}`)
  return port
}

/** Loopback origin of the gateway for `stage`, with no trailing slash. */
export function gatewayOrigin(stage: Stage): string {
  return `http://127.0.0.1:${gatewayPort(stage)}`
}
