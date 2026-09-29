/**
 * Where the keyboard's place in the agents overview goes: the arrow keys walk
 * the listed sessions in reading order, and answering a request moves on to
 * the next one still waiting, so a run of requests is cleared without
 * reaching for the pointer.
 */

export type Step = "next" | "previous" | "first" | "last"

/**
 * The id a step lands on from `current`, held at the ends; the first listed
 * when `current` is not listed (it went away, or nothing had the keyboard).
 * `null` when nothing is listed.
 */
export function stepFrom(
  order: readonly string[],
  current: string | null,
  step: Step,
): string | null {
  if (order.length === 0) return null
  const at = current === null ? -1 : order.indexOf(current)
  if (step === "first") return order[0]
  if (step === "last") return order[order.length - 1]
  if (at < 0) return order[0]
  const to = step === "next" ? at + 1 : at - 1
  return order[Math.max(0, Math.min(order.length - 1, to))]
}

/**
 * Where the keyboard goes once `answered` is answered: the next request below
 * it still waiting, else the nearest above. `null` once none is left — the
 * keyboard rests on the list, which then says nothing needs the person,
 * rather than jumping down the page to something else. `settling` are
 * requests answered and not yet gone, `answered` among them or not.
 */
export function afterAnswer(
  requests: readonly string[],
  answered: string,
  settling: ReadonlySet<string>,
): string | null {
  const open = (id: string) => id !== answered && !settling.has(id)
  const at = requests.indexOf(answered)
  if (at < 0) return requests.find(open) ?? null
  return (
    requests.slice(at + 1).find(open) ??
    requests.slice(0, at).reverse().find(open) ??
    null
  )
}

/**
 * How long after the keyboard moves on by answering that a key which answers
 * or opens is not taken, in milliseconds: a press made before the person
 * could see where the keyboard went is not a choice about the next request.
 */
export const answerPause = 250

/**
 * Whether a key press may answer (or open) the request it lands on: never a
 * held key's repeat — one press, one act, the rule the approval card's
 * answers and the composer's Return keep too — and not within
 * `answerPause` of the keyboard moving on by answering (`movedAt`, on the
 * same clock as `at`).
 */
export function takesAnswerKey(
  press: { readonly repeat: boolean; readonly at: number },
  movedAt: number | null,
): boolean {
  return !press.repeat && (movedAt === null || press.at - movedAt >= answerPause)
}
