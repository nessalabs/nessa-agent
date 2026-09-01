import { RandomAvatar } from "@nessa-ui/react/random-avatar"

import { AGENT_HUES } from "../model"

export function EmptyState({
  seed,
  ground,
  animateMount,
}: {
  seed: string
  ground: "paper" | "ink"
  animateMount: boolean
}) {
  return (
    <div className="flex flex-col items-center gap-2.5 py-6 text-center">
      <RandomAvatar
        seed={seed}
        hues={AGENT_HUES}
        name="Nessa"
        ground={ground}
        animateOnMount={animateMount}
        className="size-14 rounded-full"
      />
      <p className="nessa-text-3 m-0 text-muted-foreground">
        Nessa is listening. Press Enter to send.
      </p>
    </div>
  )
}
