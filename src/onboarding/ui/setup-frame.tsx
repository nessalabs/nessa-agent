import * as React from "react"
import {
  MorphingMeshGradient,
  morphingMeshGradientPresets,
} from "@nessa-ui/react/morphing-mesh-gradient"

/** The id the setup window's `aria-labelledby` points at. Every step titles the
 * dialog with its own heading, so there is exactly one of these on screen. */
export const SETUP_HEADING_ID = "nessa-setup-heading"

/** Every button in a setup pane: one height, one width, one shape. */
export const PANE_BUTTON = "h-11 w-full rounded-full nessa-text-3 font-medium"

/**
 * The wash every setup step is painted on.
 *
 * It is decorative: inert to the pointer, hidden from assistive technology by
 * the component, and still under a reduced-motion preference. Its pigments
 * travel, so anything laid straight on it has to stay legible wherever they
 * go — plain white type over the palette's own lighter stops measures about
 * 1.7:1, which is why the type carries a stacked shadow rather than one soft
 * one: a tight, near-opaque layer close to the glyph for an edge that holds
 * regardless of what is behind it, and a wider, softer one for depth.
 */
export function SetupStage({ children }: { children: React.ReactNode }) {
  return (
    <MorphingMeshGradient
      colors={morphingMeshGradientPresets.glass}
      type="mesh"
      speed={1.1}
      blur={88}
      className="nessa-setup-stage size-full"
    >
      {/* Setup's controls are always the light treatment: a dark pill over
        these pigments reads as a hole punched in the wash, and the light
        palette is what the design system's own components are built against
        here. Headings set their colour explicitly for the same reason. */}
      <div className="nessa-setup-light relative flex size-full min-h-0 items-center justify-center p-5">
        {children}
      </div>
    </MorphingMeshGradient>
  )
}

/**
 * The glass pane for a step that carries more than a line and a button.
 *
 * A list of options cannot be read off the moving wash, so it gets a surface.
 * The pane is scoped to the light palette because a dark slab over these
 * pigments reads as a hole rather than glass, and because its own components
 * then keep the contrast they were built for.
 *
 * It scrolls rather than clips. At a large text size or a high UI scale the
 * list and its Continue button are taller than the window, and a pane that
 * hid the button made setup impossible to finish.
 */
export const SetupPanel = React.forwardRef<HTMLDivElement, { children: React.ReactNode }>(
  function SetupPanel({ children }, ref) {
    return (
      <div
        ref={ref}
        tabIndex={-1}
        className="nessa-setup-light relative flex max-h-full w-full max-w-sm flex-col gap-5 overflow-y-auto rounded-2xl border border-white/40 bg-background/60 p-6 text-foreground shadow-2xl ring-1 ring-black/5 outline-none backdrop-blur-2xl backdrop-saturate-150"
      >
        {/* The lit top edge that reads as a pane of glass catching light. */}
        <span
          aria-hidden="true"
          className="pointer-events-none absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-white/45 to-transparent"
        />
        {children}
      </div>
    )
  },
)
