import {
  MorphingMeshGradient,
  morphingMeshGradientPresets,
} from "@nessa-ui/react/morphing-mesh-gradient"

/**
 * The presence the setup window wakes as, and then opens out of.
 *
 * There is one substance from the first frame to the last: the panel's own
 * wash, small and heavily blurred to begin with, losing its blur and growing
 * to the panel's bounds. Nothing crossfades, so the glow cannot read as an
 * unrelated gradient being swapped in for it — it *is* the background, seen
 * before it has taken its shape.
 *
 * The overlapping fields moving on their own paths are the gradient's own
 * mesh nodes rather than lobes stacked here; under that much blur they are
 * what gives the silhouette its drift. The blur is stage one's whole
 * character: no outline, no edge, nothing recognisable yet.
 *
 * Decorative throughout, and inert under a reduced-motion preference — the
 * panel is simply there.
 */
export function AgentBloom({
  colors = morphingMeshGradientPresets.glass,
}: {
  /** The palette, which must be the panel's own for the opening to be
   * continuous. Taken as a prop so a surface can be retinted in one place. */
  colors?: readonly string[]
}) {
  return (
    <div aria-hidden="true" className="nessa-setup-orb">
      <div className="nessa-setup-orb-wash">
        <MorphingMeshGradient
          colors={colors}
          type="mesh"
          // Faster than the panel's settled wash: the form is only itself for
          // a second and a half, and colour that has not moved in that time is
          // what makes an opening look like a still image.
          speed={2.4}
          blur={88}
          className="size-full"
        />
      </div>
    </div>
  )
}
