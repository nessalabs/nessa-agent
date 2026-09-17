import { AGENT_ICON_WASH } from "../../conversation"

/**
 * The agent's own colours, as the light setup opens from.
 *
 * Setup does not begin on an anonymous glow: it begins on Nessa's face. These
 * are the washes the avatar and the app icon are painted with, arranged the way
 * that painting arranges them — the warm note centred, the lavender pushed out
 * to a rim — so the first thing a person sees is the same identity that will be
 * sitting in the panel afterwards.
 *
 * It sits over the wash and fades as the box opens, which is what makes the
 * agent's colours appear to diffuse into the gradient rather than be replaced
 * by it. Decorative throughout.
 */
export function AgentBloom() {
  const [lavender, pink, peach, warm] = AGENT_ICON_WASH
  return (
    <span
      aria-hidden="true"
      className="nessa-setup-agent-bloom"
      style={{
        backgroundImage: [
          `radial-gradient(circle at 50% 50%, ${warm} 0%, transparent 46%)`,
          `radial-gradient(circle at 38% 58%, ${peach} 0%, transparent 52%)`,
          `radial-gradient(circle at 62% 40%, ${pink} 0%, transparent 58%)`,
          `radial-gradient(circle at 50% 50%, ${lavender} 0%, transparent 78%)`,
        ].join(", "),
      }}
    />
  )
}
