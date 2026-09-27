/**
 * What one read of a conversation says about the answer an end-to-end run is
 * waiting for.
 *
 * Its own module, and pure, because this is the part of `e2e-agent.mjs` worth
 * testing and the only part that can be: the rest provisions a gateway and
 * starts a real vendor subprocess, and a live run cannot be asked to produce a
 * failed turn, an empty answer or a wrong one on demand. Here they are three
 * ordinary inputs.
 */

/** What the agent is asked, and the word its answer has to contain. */
export const PROMPT = "Reply with exactly the word: pong"
export const EXPECTED = "pong"

/**
 * The model's own words in a turn.
 *
 * Text parts only. A thought is not an answer and a tool observation carries no
 * text at all, so a turn that thought at length and said nothing must not read
 * as one that replied.
 */
export function replyText(turn) {
  return turn.parts
    .filter((part) => part.kind === "text")
    .map((part) => part.text)
    .join("")
}

/**
 * Read one conversation view as `waiting`, `answered`, or `failed`.
 *
 * The distinction that matters is between a turn that is still going and a turn
 * that finished without answering. Both leave the view without the reply, and
 * only one of them is worth waiting longer for; an end-to-end check that cannot
 * tell them apart reports a gateway that never answered as a success.
 *
 * `completed` is not `answered`, for the same reason: the run asks for a
 * specific word, so a turn that completed carrying no text, or carrying
 * something else entirely, is a path that ran without working.
 */
export function verdict(view, prompt = PROMPT, expected = EXPECTED) {
  const turn = view?.messages?.find((message) => message.userText === prompt)
  if (!turn) return { state: "waiting", detail: "the turn has not appeared yet" }
  if (turn.status === "failed" || turn.status === "cancelled")
    return {
      state: "failed",
      detail: `the turn ${turn.status}: ${turn.error ?? "no reason given"}`,
    }
  if (turn.status !== "completed")
    return { state: "waiting", detail: `turn ${turn.status}` }
  const reply = replyText(turn)
  if (!reply.toLowerCase().includes(expected.toLowerCase()))
    return {
      state: "failed",
      detail: `the turn completed without saying "${expected}": ${reply || "(no text)"}`,
    }
  return { state: "answered", reply }
}
