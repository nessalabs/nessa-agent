import { css, selectorFor } from "./selectors.mjs"

/** Reads the focused window conversation's identity, once its row is published. */
export async function selectedStressConversation(page, title) {
  return page
    .locator(css.selectedSessionListRow, { hasText: title })
    .evaluateAll((rows) =>
      rows.length === 1 ? rows[0].getAttribute("data-session-row") : null,
    )
}

/** Opens a stress conversation, awaiting its selection and a composer together. */
export async function openStressConversation(page, conversationId, title) {
  await page
    .locator(selectorFor.sessionListRowFor(conversationId), { hasText: title })
    .click({ timeout: 15_000 })
  // A composer can still belong to the previous chat. Read both facts in
  // one observation: a selection that later reverted cannot satisfy a
  // composer that only appeared afterward. Never retry the click.
  await page.waitForFunction(
    ({ conversationId, selected, composer }) => {
      const boxes = document.querySelectorAll(composer)
      if (boxes.length > 1)
        throw new Error("Opening a stress conversation found ambiguous composers")
      const rows = document.querySelectorAll(selected)
      if (
        rows.length !== 1 ||
        rows[0].getAttribute("data-session-row") !== conversationId ||
        boxes.length !== 1
      )
        return false
      const box = boxes[0]
      const rect = box.getBoundingClientRect()
      return (
        rect.width > 0 &&
        rect.height > 0 &&
        getComputedStyle(box).visibility === "visible"
      )
    },
    {
      conversationId,
      selected: css.selectedSessionListRow,
      composer: `${css.conversationDock} textarea`,
    },
    { timeout: 15_000 },
  )
}
