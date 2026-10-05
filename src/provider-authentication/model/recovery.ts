/** Local submission causality is relative to an observed input identity, never a clock. */
export type ObservedInput = string | null

/** `null` captures no published input; an absent field is a projected, nonlocal input.
 * One recovery owner for both surfaces, including inputs published as pending. */
export function offersAuthenticationRecovery(
  refusedInput: string | undefined,
  publishedInput: string | undefined,
  localInputs: readonly { observedInput?: ObservedInput }[],
): boolean {
  return (
    refusedInput !== undefined &&
    refusedInput === publishedInput &&
    !localInputs.some((input) => input.observedInput === publishedInput)
  )
}
