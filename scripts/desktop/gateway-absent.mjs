/**
 * What `pnpm app` prints when the local server is not listening.
 *
 * The sentence is `startup-refusals.json` (`not-listening`). The host and
 * port are named beside it so a second start can see which socket is quiet.
 */

/**
 * @param {{ listening: boolean, port: number, stage: string, sentence: string }} input
 * @returns {string | null}
 */
export function gatewayAbsentNotice({ listening, port, stage, sentence }) {
  if (listening) return null
  if (typeof sentence !== "string" || sentence.length === 0)
    throw new Error("gateway absence notice needs its sentence")
  return `${sentence} (127.0.0.1:${port}, stage ${stage})`
}
