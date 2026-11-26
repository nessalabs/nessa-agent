/**
 * Settings pages the desktop checks open the way a person does.
 */
import { css, keys, names } from "./selectors.mjs"

/** Settings › Connections › Accounts. Shared by the vault and stress checks. */
export async function openAccounts(page) {
  if (!(await page.$(css.settings))) await page.keyboard.press(keys.settings)
  await page.locator(css.settingsCategory, { hasText: names.connections }).click()
  // Connections opens on Accounts, whose unlock dialog (a locked vault)
  // covers the tabs: click the tab only when another one is showing.
  const tab = page.locator(css.settingsTab, { hasText: names.accounts })
  await tab.waitFor()
  if ((await tab.getAttribute("aria-selected")) !== "true") await tab.click()
}

/**
 * A row's choice is a menu (`ChoiceMenu`, #1340): open it from the button in
 * `scope`, then pick the item whose `data-value` is `value`.
 */
export async function pickChoice(page, scope, value) {
  await scope.locator("[data-choice-trigger]").click()
  await page.locator(`[role="menuitemradio"][data-value="${value}"]`).click()
  await page.locator('[role="menu"]').waitFor({ state: "hidden", timeout: 5_000 })
}
