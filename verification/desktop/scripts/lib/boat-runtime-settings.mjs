import { writeFileSync } from "node:fs"
import { join } from "node:path"
import { waitFor } from "./gateway-stack.mjs"
import { css, keys, names } from "./selectors.mjs"
import { unlockRequestedVault } from "./vault.mjs"

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
    // Accounts also reads protected GitHub status. A prior listener restart
    // seals the app lease; answer that independent request through the UI.
    await unlockRequestedVault(client, page)
    const global = page.locator(css.boatRuntimeSettings)
    await global.locator("select").selectOption("14400")
    const defaultSaved = await waitFor(
      async () =>
        (await client.boat.runtimeSettings()).defaultLifetime.seconds === 14_400,
      10_000,
    )
    if (!defaultSaved)
      failures.push("the global VM lifetime was not saved through settings")
    receipts.default = await client.boat.runtimeSettings()
    // A choice saves as it is made; the select is live again once it lands.
    await waitFor(() => global.locator("select").isEnabled(), 10_000)
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
    if (
      !(await waitFor(
        async () =>
          (await client.boat.runtimeSettings({ channelId: channel.channelId }))
            .channelOverride?.mode === "never",
        10_000,
      ))
    )
      failures.push("the channel's explicit unlimited lifetime was not saved")
    await waitFor(() => select.isEnabled(), 10_000)
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

/** The channel as the gateway lists it now. */
async function listedChannel(client, channelId) {
  const { channels } = await client.channels.list()
  return channels.find((each) => each.channelId === channelId)
}

/**
 * Changes where a made channel runs from its settings dialog, as a person
 * does, after it has been talked to. New threads move first: the main chat
 * stays the same one. Then the main agent moves: the channel names a fresh
 * main chat and the one already talked to stays in the channel as a thread.
 * Both go back to this machine at the end, which names yet another fresh main
 * chat, so the channel is left running where it started. Nothing is sent
 * while it is on Boat: the run has no Boat account.
 */
export async function channelPlacementSettings(run, channel, failures) {
  const { page, client } = run
  const before = await listedChannel(client, channel.channelId)
  if (!before) {
    failures.push(
      `channel ${channel.channelId} is not listed before its run location changes`,
    )
    return
  }
  const receipts = { before }
  // The default channel has no run location to change, and an unknown one
  // is not the caller's to change.
  const refused = await client.channels
    .setPlacement({
      requestId: `stress-place-unknown-${channel.channelId}`,
      channelId: "ch_aaaaaaaaaa",
      coordinatorPlacement: "boat",
      threadPlacement: "boat",
    })
    .then(
      () => "accepted",
      (error) => error.code,
    )
  if (refused !== "forbidden")
    failures.push(`changing an unknown channel's run location answered ${refused}`)
  const gear = page.locator(
    `${css.channelSettingsOpen}[data-channel-settings-open="${channel.channelId}"]`,
  )
  const openSettings = async () => {
    await page
      .locator(`${css.sidebar} nav`)
      .getByText(channel.name, { exact: true })
      .first()
      .hover()
    await gear.click()
  }
  await openSettings()
  const dialog = page.locator(css.channelSettingsDialog)
  const control = dialog.locator(`[data-channel-placement="${channel.channelId}"]`)
  const attribute = {
    coordinator: "data-coordinator-placement",
    threads: "data-thread-placement",
  }
  const field = { coordinator: "coordinatorPlacement", threads: "threadPlacement" }
  const option = (row, name) =>
    control
      .locator(`[data-placement-row="${row}"]`)
      .getByRole("button", { name, exact: true })
  // A choice is done when the dialog shows it, not when it stops being busy:
  // a save not started yet is not busy either.
  const shows = (row, place) =>
    waitFor(async () => (await control.getAttribute(attribute[row])) === place, 10_000)
  // Boat is chosen in the dialog only while it is connected. Otherwise it is
  // offered, disabled, the move is asked of the gateway directly, as another
  // window would, and the dialog is checked once the window reads it again.
  await control.waitFor({ timeout: 10_000 })
  const boatOffered = (await option("threads", "Boat").count()) > 0
  const toBoat = async (row, current) => {
    if (!boatOffered) {
      await client.channels.setPlacement({
        requestId: `stress-place-${row}-${channel.channelId}`,
        channelId: channel.channelId,
        ...current,
        [field[row]]: "boat",
      })
      return
    }
    await option(row, "Boat").click()
    if (!(await shows(row, "boat")))
      failures.push(
        `the channel settings do not show ${row === "threads" ? "new threads" : "the main agent"} on Boat`,
      )
  }
  const toMac = async (row) => {
    await option(row, "This Mac").click()
    if (!(await shows(row, "local")))
      failures.push(`choosing This Mac for ${row} did not show in the channel settings`)
  }
  const settled = (want) =>
    waitFor(async () => {
      const now = await listedChannel(client, channel.channelId)
      return now &&
        now.coordinatorPlacement === want.coordinatorPlacement &&
        now.threadPlacement === want.threadPlacement
        ? now
        : null
    }, 10_000)
  try {
    if (
      (await control.getAttribute("data-coordinator-placement")) !==
      before.coordinatorPlacement
    )
      failures.push("the channel settings do not show where its main agent runs")
    receipts.boatOffered = boatOffered
    if (!boatOffered) {
      const waiting = option("threads", "Boat (not connected)")
      if ((await waiting.count()) === 0 || !(await waiting.isDisabled()))
        failures.push("Boat is not shown as unavailable while it is not connected")
    }
    await toBoat("threads", {
      coordinatorPlacement: before.coordinatorPlacement,
      threadPlacement: before.threadPlacement,
    })
    const threads = await settled({
      coordinatorPlacement: before.coordinatorPlacement,
      threadPlacement: "boat",
    })
    receipts.threads = threads
    if (!threads) failures.push("moving new threads to Boat was not saved")
    else if (threads.coordinatorConversationId !== before.coordinatorConversationId)
      failures.push("moving only new threads named a new main chat")
    await toBoat("coordinator", {
      coordinatorPlacement: before.coordinatorPlacement,
      threadPlacement: "boat",
    })
    const moved = await settled({ coordinatorPlacement: "boat", threadPlacement: "boat" })
    receipts.moved = moved
    if (!moved) failures.push("moving the main agent to Boat was not saved")
    else {
      if (moved.coordinatorConversationId === before.coordinatorConversationId)
        failures.push(
          "moving the main agent kept the main chat that runs on this machine",
        )
      // The main chat already talked to stays in the channel, as a thread.
      const { conversations } = await client.conversation.list({})
      const old = conversations.find(
        (each) => each.conversationId === before.coordinatorConversationId,
      )
      if (!old) failures.push("the old main chat is gone after the main agent moved")
      else if (old.channelId !== channel.channelId)
        failures.push("the old main chat left its channel when the main agent moved")
    }
    if (!boatOffered) {
      await page.keyboard.press(keys.escape)
      await page.reload()
      await openSettings()
      for (const row of ["coordinator", "threads"])
        if (!(await shows(row, "boat")))
          failures.push(`after a reload the channel settings do not show ${row} on Boat`)
    }
    if (run.evidence)
      await page.screenshot({
        path: join(run.evidence, `channel-placement-${channel.channelId}.png`),
      })
    await toMac("coordinator")
    await toMac("threads")
    const back = await settled({
      coordinatorPlacement: "local",
      threadPlacement: "local",
    })
    receipts.back = back
    if (!back) failures.push("moving the channel back to this machine was not saved")
    else if (moved) {
      // A Boat main chat that was made is retired and stays a thread. One not
      // listed may still have been begun (the window opens it, and its start
      // fails without Boat), which the gateway counts as made, so either
      // answer holds for it.
      const { conversations } = await client.conversation.list({})
      const made = conversations.some(
        (each) => each.conversationId === moved.coordinatorConversationId,
      )
      receipts.boatMainMade = made
      receipts.boatMainReplaced =
        back.coordinatorConversationId !== moved.coordinatorConversationId
      if (made && !receipts.boatMainReplaced)
        failures.push("moving the main agent back kept the Boat main chat")
    }
    await page.keyboard.press(keys.escape)
  } finally {
    if (run.evidence)
      writeFileSync(
        join(run.evidence, `channel-placement-${channel.channelId}.json`),
        JSON.stringify(receipts, null, 2) + "\n",
      )
  }
}
