import { css, names, preferenceEvents, selectorFor, storage } from "./selectors.mjs"

/** Read semantic chat identities even after the renderer replaces UUIDs with titles. */
export function chatIdentityTexts(nodes) {
  return nodes.map((node) => {
    const copy = node.cloneNode(true)
    for (const pill of copy.querySelectorAll("[data-chat-pill]"))
      pill.replaceWith(pill.getAttribute("data-chat-pill"))
    return copy.textContent ?? ""
  })
}

/**
 * Chooses the model named exactly `displayName` in the model picker that
 * `trigger` opens, by searching for it: whatever the picker opens on (Starred,
 * the provider in use), only that one model is chosen, and a name it does not
 * list once is refused rather than another taken.
 */
export async function selectStressModel(page, trigger, displayName) {
  await trigger.click()
  const search = page.locator(css.modelSearch)
  await search.waitFor({ timeout: 10_000 })
  await search.fill(displayName)
  const choice = page.locator(css.modelOption).filter({
    has: page.locator(css.modelOptionLabel).getByText(displayName, { exact: true }),
  })
  if ((await choice.count()) !== 1)
    throw new Error("the exact fixture model is not uniquely offered in Home")
  await choice.locator(css.modelOptionLabel).click()
}

/**
 * Takes every star off the model picker, in storage and in the open window,
 * so a check that stars a model starts and ends with none, even after a run
 * that stopped halfway.
 */
export async function clearStarredModels(page) {
  await page.evaluate(
    ([key, event]) => {
      try {
        localStorage.removeItem(key)
      } catch {
        // Nothing kept to take off.
      }
      window.dispatchEvent(new CustomEvent(event, { detail: null }))
    },
    [storage.starredModels, preferenceEvents.starredModels],
  )
}

/** Reads the focused window conversation's identity, once its row is published. */
export async function selectedStressConversation(page, title) {
  return page
    .locator(css.stressSelectedSessionRow, { hasText: title })
    .evaluateAll((rows) =>
      rows.length === 1 ? rows[0].getAttribute("data-session-row") : null,
    )
}

/** Opens a stress conversation, awaiting its selection and a composer together. */
export async function openStressConversation(page, conversationId, title) {
  const columns = await page.locator(css.sessionList).count()
  const row = () =>
    page.locator(
      columns
        ? selectorFor.sessionListRowFor(conversationId)
        : selectorFor.stressTreeSessionFor(conversationId),
      { hasText: title },
    )
  if (!columns && !(await row().count())) {
    // Sessions-in-sidebar has no workspace-list. Disclose the actual tree,
    // then its bounded older rows, without opening an unrelated conversation.
    const branches = page.locator(css.stressTreeChannel)
    for (let index = 0; index < (await branches.count()); index += 1) {
      const branch = branches.nth(index)
      if ((await branch.getAttribute("aria-expanded")) === "false")
        await branch.locator(css.stressTreeDisclosure).click({ timeout: 15_000 })
    }
    if (!(await row().count())) {
      const more = page.locator(css.stressTreeMore).filter({ hasText: names.showAll })
      const count = await branches.count()
      for (let shown = 0; shown < count && (await more.count()); shown += 1)
        await more.first().click({ timeout: 15_000 })
    }
  }
  await row().click({ timeout: 15_000 })
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
      selected: css.stressSelectedSessionRow,
      composer: `${css.conversationDock} textarea`,
    },
    { timeout: 15_000 },
  )
}
