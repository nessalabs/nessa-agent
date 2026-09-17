import {
  MorphingMeshGradient,
  morphingMeshGradientPresets,
} from "@nessa-ui/react/morphing-mesh-gradient"

/**
 * The blob setup opens from, and then opens into.
 *
 * It is made of the same wash the box is — the same component, the same
 * palette — so the opening is one substance changing shape rather than two
 * things crossfading. Taking the icon's idea of a fluid lit body, but in the
 * gradient's own colours, is what lets that be literal rather than a
 * resemblance.
 *
 * The wash is deliberately *not* carried by the body. The body rolls and
 * grows; the colour behind it holds still, so the window is uncovered rather
 * than magnified — a wash that grows with its container reads as a zoom, which
 * is what the first attempt at this looked like. Both transforms are driven
 * from one animated custom property and the wash applies its exact inverse, so
 * they cancel at every frame rather than only at the keyframes.
 *
 * Decorative throughout.
 */
export function AgentBloom() {
  return (
    <div aria-hidden="true" className="nessa-setup-orb">
      <div className="nessa-setup-orb-wash">
        <MorphingMeshGradient
          colors={morphingMeshGradientPresets.glass}
          type="mesh"
          speed={1.1}
          blur={88}
          className="size-full"
        />
      </div>
      {/* What makes it a body: lit from the upper left, falling away at the
        lower right. It scales with the body, so the lighting stays in
        proportion to it, and it leaves as the body flattens into a window. */}
      <span className="nessa-setup-orb-shade" />
    </div>
  )
}
