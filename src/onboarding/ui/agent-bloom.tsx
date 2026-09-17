import { RandomAvatar } from "@nessa-ui/react/random-avatar"

import { AGENT_HUES, AGENT_SEED } from "../../conversation"

/**
 * The agent itself, holding the screen before setup opens.
 *
 * This is not a glow shaped like the avatar — it is the avatar, the same
 * deterministic painting the header and the app icon are, at the size of a
 * planet. So the first thing a person sees is literally the face that will be
 * sitting in the panel afterwards, and there is no second definition of it to
 * drift: seed and hue wheel come from `identity.ts` like everywhere else.
 *
 * Its own paint does the living: `animateOnMount` blooms the pools on, `busy`
 * keeps a wash flooding and handing over to the next. What is added here is
 * volume — the paint is lit from the upper left and falls away at the lower
 * right, and it turns under a fixed highlight, which is what makes a flat
 * painting read as a sphere. Decorative throughout.
 */
export function AgentBloom() {
  return (
    <div aria-hidden="true" className="nessa-setup-orb">
      <RandomAvatar
        seed={AGENT_SEED}
        hues={AGENT_HUES}
        ground="paper"
        animateOnMount
        busy
        speed={1.8}
        className="nessa-setup-orb-face size-full rounded-full"
      />
      {/* The highlight stays put while the paint turns beneath it. */}
      <span className="nessa-setup-orb-sheen" />
    </div>
  )
}
