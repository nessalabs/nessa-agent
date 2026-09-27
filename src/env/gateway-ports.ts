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

const stages: Readonly<Record<Stage, number>> = table.stages

/**
 * Whether the table names this stage itself.
 *
 * `Object.hasOwn`, not `in` or a plain read. The table is an object like any
 * other, so it answers to `toString`, `constructor` and `__proto__` with
 * something it inherited: `"toString" in stages` is true and `stages.toString`
 * is a function, neither of which is a port. `NESSA_STAGE=toString` walked
 * through the old guard and came out the other side as
 * `http://127.0.0.1:function toString() { [native code] }`. Only a key the
 * table owns is a stage.
 */
function isStage(named: string): named is Stage {
  return Object.hasOwn(stages, named)
}

/**
 * The stage a value names, or a refusal.
 *
 * Matched exactly, because the server's `Stage::parse` matches exactly: a rule
 * more forgiving here reads `Dev` as dev, resolves a port, and hands it to a
 * server that refuses to start on a stage it does not recognise. Surrounding
 * whitespace is a shell artefact and is not a different stage; an empty value
 * is how a shell spells unset and is the dev stage, like an absent one.
 *
 * This is the boundary that turns an environment variable into a `Stage`, so it
 * is where the narrowing belongs — everything downstream takes the union.
 */
export function parseStage(value: string | undefined): Stage {
  const named = (value ?? "").trim()
  if (named === "") return "dev"
  if (!isStage(named))
    throw new Error(
      `NESSA_STAGE=${named} is not a stage. ` +
        `The server accepts exactly: ${Object.keys(stages).join(", ")}.`,
    )
  return named
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
  // A second `Object.hasOwn`, because `Stage` is only as good as the narrowing
  // that produced it and a cast produces none. The old `port === undefined`
  // read as this check but was not one: an inherited member is not undefined.
  if (!isStage(stage)) throw new Error(`No gateway port for stage ${stage}`)
  return stages[stage]
}

/** Loopback origin of the gateway for `stage`, with no trailing slash. */
export function gatewayOrigin(stage: Stage): string {
  return `http://127.0.0.1:${gatewayPort(stage)}`
}
