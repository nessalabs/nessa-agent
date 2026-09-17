import {
  MorphingMeshGradient,
  morphingMeshGradientPresets,
} from "@nessa-ui/react/morphing-mesh-gradient"

/**
 * The blob setup opens from, and then opens into.
 *
 * It is made of the same wash the box is — the same component, the same
 * palette — so the opening is one substance changing shape rather than two
 * things crossfading: a blob rolls in the dark, settles, and then squares
 * itself off into the window. Taking the icon's idea (a fluid wash with a warm
 * centre) but in the gradient's own colours is what lets that be literal.
 *
 * Volume comes from lighting, not from blurring it: a highlight fixed at the
 * upper left, the surface falling into shadow at the lower right. Both leave
 * as it flattens into the box, because a window is not a sphere. Decorative
 * throughout.
 */
export function AgentBloom() {
  return (
    <div aria-hidden="true" className="nessa-setup-orb">
      <div className="nessa-setup-orb-body">
        <MorphingMeshGradient
          colors={morphingMeshGradientPresets.glass}
          type="mesh"
          speed={1.1}
          blur={88}
          className="size-full"
        />
        {/* The specular highlight, and the terminator under it. */}
        <span className="nessa-setup-orb-sheen" />
      </div>
    </div>
  )
}
