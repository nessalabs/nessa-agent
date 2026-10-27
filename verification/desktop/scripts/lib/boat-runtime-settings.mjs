import { writeFileSync } from "node:fs"
import { join } from "node:path"
import { waitFor } from "./gateway-stack.mjs"
import { css, keys, names } from "./selectors.mjs"

/** Settings › Connections › Accounts. Shared by runtime and subscription checks. */
export async function openAccounts(page) {
  if (!(await page.$(css.settings))) await page.keyboard.press(keys.settings)
  await page.locator(css.settingsCategory, { hasText: names.connections }).click()
  await page.locator(css.settingsTab, { hasText: names.accounts }).click()
}

/** Edit the global default and one saved channel through the actual UI,
 * without a Boat token. The client readbacks prove config persistence and
 * distinguish inheritance from an explicit no-auto-stop override. */
export async function channelRuntimeSettings(run, channel, failures) {
  const { page, client } = run
  const before = await client.boat.runtimeSettings()
  const receipts = { before }
  for (const params of [
    { mode: "never", channelId: null },
    { mode: "never", seconds: null },
    { mode: "never", seconds: 3600 },
    { mode: "default", seconds: 3600 },
    { mode: "after" },
    { mode: "after", seconds: 0 },
    { mode: "after", seconds: 4294967296 },
  ]) {
    const refused = await client.boat.setRuntimeSettings(params).then(
      () => undefined,
      (error) => error.refusal,
    )
    if (refused !== "invalid_request")
      failures.push(`invalid runtime choice was not refused: ${JSON.stringify(params)}`)
  }
  const unchanged = await client.boat.runtimeSettings()
  if (JSON.stringify(unchanged) !== JSON.stringify(before))
    failures.push("invalid runtime choices changed the saved default")
  try {
    await openAccounts(page)
    const global = page.locator(css.boatRuntimeSettings)
    await global.locator("select").selectOption("14400")
    await global.getByRole("button", { name: "Save VM settings", exact: true }).click()
    const defaultSaved = await waitFor(
      async () =>
        (await client.boat.runtimeSettings()).defaultLifetime.seconds === 14_400,
      10_000,
    )
    if (!defaultSaved)
      failures.push("the global VM lifetime was not saved through settings")
    receipts.default = await client.boat.runtimeSettings()
    await global.getByRole("button", { name: "Save VM settings", exact: true }).waitFor()
    if (run.evidence)
      await page.screenshot({
        path: join(run.evidence, `boat-default-${channel.channelId}.png`),
      })
    await page.getByRole("button", { name: names.leaveSettings }).first().click()
    await page
      .locator(`${css.sidebar} nav`)
      .getByText(channel.name, { exact: true })
      .first()
      .hover()
    const gear = page.locator(
      `${css.channelSettingsOpen}[data-channel-settings-open="${channel.channelId}"]`,
    )
    const geometry = await gear.evaluate((button) => {
      const row = button.closest('[data-slot="sidebar-menu-item-row"]')
      const label = row?.querySelector('[data-slot="sidebar-menu-item-label"]')
      const control = button.getBoundingClientRect()
      const hit = document.elementFromPoint(
        control.x + control.width / 2,
        control.y + control.height / 2,
      )
      return {
        labelClear: !!label && label.getBoundingClientRect().right <= control.left,
        reachable: hit === button || button.contains(hit),
      }
    })
    if (!geometry.labelClear || !geometry.reachable)
      failures.push(
        "channel settings control overlaps its label or cannot receive a click",
      )
    await gear.click()
    const dialog = page.locator(css.channelSettingsDialog)
    const control = dialog.locator(css.boatRuntimeSettings)
    const select = control.locator("select")
    await waitFor(async () => (await select.inputValue()) === "default", 10_000)
    if ((await select.inputValue()) !== "default")
      failures.push("a new channel did not inherit the VM lifetime default")
    await select.selectOption("never")
    await control.getByRole("button", { name: "Save VM settings", exact: true }).click()
    if (
      !(await waitFor(
        async () =>
          (await client.boat.runtimeSettings({ channelId: channel.channelId }))
            .channelOverride?.mode === "never",
        10_000,
      ))
    )
      failures.push("the channel's explicit unlimited lifetime was not saved")
    await control.getByRole("button", { name: "Save VM settings", exact: true }).waitFor()
    receipts.override = await client.boat.runtimeSettings({
      channelId: channel.channelId,
    })
    if (receipts.override.defaultLifetime.seconds !== 14_400)
      failures.push("a channel override changed the global default")
    if (run.evidence)
      await page.screenshot({
        path: join(run.evidence, `boat-channel-${channel.channelId}.png`),
      })
    await select.selectOption("default")
    await control.getByRole("button", { name: "Save VM settings", exact: true }).click()
    if (
      !(await waitFor(
        async () =>
          (await client.boat.runtimeSettings({ channelId: channel.channelId }))
            .channelOverride === undefined,
        10_000,
      ))
    )
      failures.push("resetting a channel did not remove its VM lifetime override")
    receipts.inherited = await client.boat.runtimeSettings({
      channelId: channel.channelId,
    })
    await page.keyboard.press(keys.escape)
    if (
      !(await waitFor(
        () => gear.evaluate((button) => document.activeElement === button),
        5000,
      ))
    )
      failures.push(
        "closing channel settings did not return keyboard focus to its control",
      )
  } finally {
    await client.boat.setRuntimeSettings({
      mode: before.defaultLifetime.mode,
      ...(before.defaultLifetime.mode === "after"
        ? { seconds: before.defaultLifetime.seconds }
        : {}),
    })
    if (run.evidence)
      writeFileSync(
        join(run.evidence, `boat-runtime-${channel.channelId}.json`),
        JSON.stringify(receipts, null, 2) + "\n",
      )
  }
}
