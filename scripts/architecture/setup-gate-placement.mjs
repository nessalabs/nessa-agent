import { normalizedPath } from "./rust-boundaries.mjs"

/** The composition whose surface becomes the panel in place. */
const BROWSER_COMPOSITION = "src/composition/browser.tsx"

/**
 * In a browser, the panel is rendered inside first-run setup and nowhere else.
 *
 * This surface has no host to write the agent choice to and no second window to
 * read it back in: the same page becomes the panel, so the gate handing the
 * choice over is the only record it ever gets. That handover is one prop on one
 * element here, and both halves of it — the gate that offers it and the
 * dependencies that keep it — are covered by their own tests. Delete this
 * element and every suite still passes while a user shown Codex as ready, who
 * picks it and finishes setup, silently gets the gateway's default.
 *
 * Checked in the source rather than by mounting it, because mounting the panel
 * means mounting the whole design system to assert one thing about one element,
 * and the assertion would then have several ways to fail that have nothing to
 * do with the claim.
 */
export function setupGatePlacementViolations(path, source) {
  if (normalizedPath(path) !== BROWSER_COMPOSITION) return []

  const open = source.match(/<BrowserSetupGate\b[^>]*>/)
  if (!open) {
    return [
      "the browser composition must render <BrowserSetupGate>; without it the agent chosen during setup reaches nothing",
    ]
  }
  const from = source.indexOf(open[0]) + open[0].length
  const to = source.indexOf("</BrowserSetupGate>", from)
  if (to === -1) {
    return ["<BrowserSetupGate> is opened here and never closed"]
  }

  const failures = []
  const inside = source.slice(from, to)
  if (!/<App\b/.test(inside)) {
    failures.push(
      "the panel must be rendered inside <BrowserSetupGate>, so setup is answered before a conversation can be created",
    )
  }
  if (!/\bonHandOver\b|dependencies=/.test(open[0])) {
    failures.push(
      "<BrowserSetupGate> must be given the dependencies that keep the chosen agent; without them the picker decides nothing",
    )
  }
  return failures
}
