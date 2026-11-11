/**
 * Whether the line under a composer shows where the conversation works — its
 * project and the computer it runs on — or only the thinking level, which
 * always shows. One choice for the window, remembered between launches
 * (`adapters/composer-details-preference.ts`).
 */
export type ComposerDetails = "shown" | "hidden"

export const defaultComposerDetails: ComposerDetails = "shown"

/** Reads a stored choice, falling back to the default for anything unknown. */
export function parseComposerDetails(value: unknown): ComposerDetails {
  return value === "hidden" || value === "shown" ? value : defaultComposerDetails
}

/** The other choice: what the line's Hide or Show button asks for. */
export function otherComposerDetails(details: ComposerDetails): ComposerDetails {
  return details === "shown" ? "hidden" : "shown"
}
