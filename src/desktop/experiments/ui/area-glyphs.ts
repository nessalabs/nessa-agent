/**
 * Each area drawn as what it works on: a line drawing on a 16-unit grid, as
 * one path so anything that draws SVG — a mark beside a name, a chart's lane,
 * the subagents panel — draws the same picture.
 */
const glyphs: Record<string, string> = {
  // Instructions: lines of text.
  prompt: "M3 4.5h10M3 8h10M3 11.5h6",
  // A wrench.
  tools:
    "M10.6 2.6a3 3 0 0 0-2.8 4L3.2 11.2a1.1 1.1 0 0 0 1.6 1.6l4.6-4.6a3 3 0 0 0 4-2.8l-1.7 1.7-1.7-.5-.5-1.7z",
  // Finding: a lens.
  retrieval: "M3.1 7a3.9 3.9 0 1 0 7.8 0a3.9 3.9 0 1 0-7.8 0M10 10l3.3 3.3",
  // A chip.
  model:
    "M6.1 4.5h3.8a1.6 1.6 0 0 1 1.6 1.6v3.8a1.6 1.6 0 0 1-1.6 1.6H6.1a1.6 1.6 0 0 1-1.6-1.6V6.1a1.6 1.6 0 0 1 1.6-1.6zM6.5 2v2.5M9.5 2v2.5M6.5 11.5V14M9.5 11.5V14M2 6.5h2.5M2 9.5h2.5M11.5 6.5H14M11.5 9.5H14",
  // The loop around the model.
  harness:
    "M12.6 7A4.7 4.7 0 0 0 4.2 4.6M4.2 2.4v2.4h2.4M3.4 9a4.7 4.7 0 0 0 8.4 2.4m0 2.2v-2.2H9.4",
}

/** A diamond, for an area with no drawing of its own. */
const fallback = "M8 2.5L13.5 8 8 13.5 2.5 8z"

export function areaGlyphPath(areaId: string | undefined): string {
  return areaId !== undefined && Object.hasOwn(glyphs, areaId) ? glyphs[areaId] : fallback
}
