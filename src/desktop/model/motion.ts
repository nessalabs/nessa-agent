/**
 * How much the window moves: as the system asks, always in full, or reduced.
 * Chosen in Settings › Appearance › Motion. What is in effect is one of two
 * — `full` or `reduced` — which the window carries on its root as
 * `data-motion`, and which the stylesheet's durations and every script that
 * moves anything read, so there is one answer to "should this move?".
 */
export const motionChoices = [
  { id: "system", label: "System" },
  { id: "full", label: "Full" },
  { id: "reduced", label: "Reduced" },
] as const

export type MotionChoice = (typeof motionChoices)[number]["id"]

/** Reads a stored or requested choice, falling back to following the system. */
export function parseMotionChoice(value: unknown): MotionChoice {
  return motionChoices.find((choice) => choice.id === value)?.id ?? "system"
}

/** The motion in effect: the person's choice, or the system's where they left it to it. */
export function motionInEffect(
  choice: MotionChoice,
  systemReduces: boolean,
): "full" | "reduced" {
  if (choice === "system") return systemReduces ? "reduced" : "full"
  return choice
}
