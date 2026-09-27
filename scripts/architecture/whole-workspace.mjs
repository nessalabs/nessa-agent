/**
 * ADR 238: nothing in the desktop window selects the workspace as a whole.
 * A selector that returns the whole slice changes identity with every change
 * to it — a streamed word, a status — so whatever renders from it renders for
 * all of them, which is the whole-window re-render the workspace vertical
 * exists to avoid. Views select what they show.
 *
 * Pure text, like every rule here: `check-architecture.mjs` runs on bare Node.
 * It refuses the shapes a selector returning the slice is written in — an
 * arrow whose whole body is `<parameter>.workspace`, typed or not, and one
 * that destructures `workspace` and returns it — wherever they appear under
 * `src/desktop/`. Reading a field (`state.workspace.panes`) is not refused,
 * and neither is a thunk's `getState().workspace`, which is read once, when
 * it runs, and renders nothing.
 */

/** The ways an arrow can return the whole slice, as source text. */
const wholeSlice = [
  // (state) => state.workspace   (state: DesktopState) => state.workspace   s => s.workspace
  /(?:\(\s*(\w+)\s*(?::[^()]*)?\)|\b(\w+))\s*=>\s*(?:\1|\2)\s*\.\s*workspace\b(?!\s*[.?[\w])/,
  // ({ workspace }) => workspace
  /\(\s*\{\s*workspace\s*\}\s*(?::[^()]*)?\)\s*=>\s*workspace\b(?!\s*[.?[\w])/,
]

export function wholeWorkspaceViolations(path, text) {
  if (!path.startsWith("src/desktop/") || /\.test\.tsx?$/.test(path)) return []
  return wholeSlice.some((pattern) => pattern.test(text))
    ? ["desktop code selects what it shows, never the whole workspace (ADR 238)"]
    : []
}
