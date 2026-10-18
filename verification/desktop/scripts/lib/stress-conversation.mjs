import { css, selectorFor } from "./selectors.mjs"

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
