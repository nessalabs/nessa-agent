# Design direction

**Soft, fluid systems.**

Nessa is a translucent panel that hangs over whatever the reader was already
looking at. Everything below follows from that one fact: the app is not a
window with a brand colour in it, it is a piece of weather sitting on someone
else's screen. The surface should feel like light in fog — diffuse, flowing,
slightly grainy — rather than like a card with a fill.

This document is the standing brief. Anything new that paints should be read
against it before it ships.

## The five ideas

1. **Diffuse gradient washes.** Warm orange blooming through lavender,
   periwinkle and blue. No hard stops, no visible ends, no banding. A wash
   should read as light diffusing through a medium, not as a gradient with a
   start and a finish. If you can point at where one colour becomes another,
   it is too tight.

2. **No line work over the wash.** Both references carry a fine mesh — a
   warped grid, a dot field — and it was tried: `GradientSurface`'s `contours`,
   `waves` and `rings` all rendered against these palettes. It was rejected on
   sight. At icon size the lines read as a diagram drawn on the colour rather
   than as tooth in the paper, and they compete with the disc for the eye. The
   wash carries the surface alone. Reach for a pattern only if a much larger
   surface later asks for one, and expect to justify it.

3. **Grain.** Film grain over the whole frame, at print strength. It is what
   stops a large soft gradient from looking like a screensaver, and what makes
   the surface feel like a material rather than a value.

4. **Frosted glass.** Panels float on the wash. Nessa's panel is already
   frosted natively by `NSVisualEffectView` (see `src-tauri/src/vibrancy.rs`),
   so this is not a style to adopt — it is what the app already is, and new
   surfaces should be continuous with it rather than opaque next to it.

5. **The circular mark.** Nessa's face is the generative `RandomAvatar` disc,
   painted from a seed and a hue wheel (`src/agent-identity.ts`). It is the
   same painting in the transcript, the tab strip and the app icon, and it
   stays circular. Where it needs a container, a soft scalloped or petalled
   one suits the direction; a hard square does not.

## What this is not

- Not flat brand colour, and not a solid fill behind content.
- Not a *dark* gradient. The reference direction is **pale** — light washes
  with soft blooms. The design system's `GradientSurface` presets are all deep
  (`dusk`, `orchid`, `ember`); this direction wants a custom light palette, not
  a preset.
- Not decoration that competes. The panel's job is a conversation. The wash
  belongs on grounds, empty states, the icon and brand surfaces — never behind
  running text.
- Not animation for its own sake. The avatars were deliberately stilled
  (`ad1539e`) because continuous SVG filter work cost real frames. Motion has
  to earn its cost.

## What already exists

`gradient-surface.tsx` in the linked `@nessa-ui/react` checkout is this
component. It takes `colors` (a palette painted as soft radial blooms),
`pattern`, `patternColor`, `patternOpacity` and `grain`. Pass
`pattern="none"` — see idea 2. What this direction uses it for is the wash and
the grain.

Import it the way every other component here is imported:

```ts
import { GradientSurface } from "@nessa-ui/react/gradient-surface"
```

## The one constraint worth knowing

`GradientSurface` paints **CSS** radial gradients plus inline SVG. The app icon
is rasterised outside a browser, by resvg (`scripts/render-icon.mjs`), which
runs no CSS and does not implement `oklch`. So the icon cannot reuse the
component: its wash has to be authored as real SVG gradients, with colours
already in sRGB. The script's own header explains the rest.

Preview surfaces are a browser, so they *can* use the component directly —
which is why `/icon.html` composes the real thing and the shipped `.png` is
rendered from a hand-authored SVG that matches it.

## The chosen wash

`AGENT_ICON_WASH` in `src/agent-identity.ts` — warm through the middle, lavender
pushed out to a rim. Picked from the candidates at `/icon.html`, with no line
work over it. It lives beside the seed and the hue wheel because the shipped
icon is authored twice — once as this component, once as SVG for resvg — and
those numbers are what the two have to agree on.

## Where it applies

The app icon first. Then empty states, and any brand surface that comes later.

One thing to decide per surface rather than by default: the **tray icon** is
16pt in a menu bar. A gradient ground at that size collapses into a coloured
square, which is the exact failure the current tray icon was tuned away from
(see the README on regenerating it). The direction applies to the app icon; it
does not automatically apply to the tray.
