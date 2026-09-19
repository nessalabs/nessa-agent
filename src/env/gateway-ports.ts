import table from "../../protocol/defaults/gateway-ports.json"

/**
 * The stages there are, taken from the table rather than written out again.
 *
 * Hand-maintained, this union drifted from the table it describes and from the
 * server's own `Stage::parse` — and a `value as Stage` cast let an unchecked
 * string past all three. Derived, adding a stage to the table is what adds it
 * here, and `parseStage` is the only way in.
 */
export type Stage = keyof typeof table.stages

const stages: Readonly<Record<string, number>> = table.stages

/**
 * The stage a value names, or a refusal.
 *
 * Matched exactly, because the server's `Stage::parse` matches exactly: a rule
 * more forgiving here reads `Dev` as dev, resolves a port, and hands it to a
 * server that refuses to start on a stage it does not recognise. Surrounding
 * whitespace is a shell artefact and is not a different stage; an empty value
 * is how a shell spells unset and is the dev stage, like an absent one.
 */
export function parseStage(value: string | undefined): Stage {
  const named = (value ?? "").trim()
  if (named === "") return "dev"
  if (!(named in stages))
    throw new Error(
      `NESSA_STAGE=${named} is not a stage. ` +
        `The server accepts exactly: ${Object.keys(stages).join(", ")}.`,
    )
  return named as Stage
}

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
