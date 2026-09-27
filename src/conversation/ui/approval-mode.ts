import type { ApprovalMode } from "../model"

/**
 * What each approval mode is called and what it lets the agent do, in the words
 * the details sheet and the composer's tray both use. A total record, so a mode
 * added to `ApprovalMode` does not compile until it has words.
 */
export const APPROVAL_MODE_TEXT: Readonly<
  Record<ApprovalMode, { name: string; says: string }>
> = Object.freeze({
  ask: { name: "Ask first", says: "Nessa asks before each tool runs." },
  auto: { name: "Automatic", says: "Routine tools run; anything risky still asks." },
  full: { name: "Full access", says: "Every tool runs without asking." },
})

/** Presentation order: most careful first. */
export const APPROVAL_MODES: readonly ApprovalMode[] = Object.freeze([
  "ask",
  "auto",
  "full",
])
