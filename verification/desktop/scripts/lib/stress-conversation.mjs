import { css, selectorFor } from "./selectors.mjs"

/** Reads the focused window conversation's identity, once its row is published. */
export async function selectedStressConversation(page, title) {
  return page
    .locator(css.selectedSessionListRow, { hasText: title })
    .evaluateAll((rows) =>
      rows.length === 1 ? rows[0].getAttribute("data-session-row") : null,
    )
}

/** Opens a stress conversation in the window, and waits for its composer. */
export async function openStressConversation(page, conversationId, title) {
  await page
    .locator(selectorFor.sessionListRowFor(conversationId), { hasText: title })
    .click({ timeout: 15_000 })
  await page
    .locator(css.conversationDock)
    .locator("textarea")
    .waitFor({ timeout: 15_000 })
}
