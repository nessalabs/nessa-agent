import { writeFileSync } from "node:fs"
import { join } from "node:path"
import { log } from "./cli.mjs"
import { waitFor } from "./gateway-stack.mjs"
import { css, keys, names } from "./selectors.mjs"
import { unlockRequestedVault } from "./vault.mjs"

/** Settings › Connections › Accounts. Shared by runtime and subscription checks. */
export async function openAccounts(page) {
  if (!(await page.$(css.settings))) await page.keyboard.press(keys.settings)
  await page.locator(css.settingsCategory, { hasText: names.connections }).click()
  // Connections opens on Accounts, whose unlock dialog (a locked vault)
  // covers the tabs: click the tab only when another one is showing.
  const tab = page.locator(css.settingsTab, { hasText: names.accounts })
  await tab.waitFor()
  if ((await tab.getAttribute("aria-selected")) !== "true") await tab.click()
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
    // Settings' sidebar is the app's flat sidebar, not a floating card.
    const sidebar = await page.locator(".settings-glass").evaluate((glass) => {
      const style = getComputedStyle(glass)
      const box = glass.getBoundingClientRect()
      return {
        left: box.left,
        top: box.top,
        radius: style.borderTopLeftRadius,
        shadow: style.boxShadow,
      }
    })
    if (
      sidebar.left !== 0 ||
      sidebar.top !== 0 ||
      sidebar.radius !== "0px" ||
      sidebar.shadow !== "none"
    )
      failures.push(`Settings' sidebar is a card: ${JSON.stringify(sidebar)}`)
    // Each account is one row: a word of state and one button. The token is
    // pasted in a dialog, never in a field in the list; Auto-stop is its own
    // row beside Boat's.
    const boat = page.locator('[data-setting="boat"]')
    // The initial read can still be pending after the vault has unlocked.
    // Connected closes a token dialog, so choose its action only once read.
    const read = await waitFor(
      async () =>
        ["connected", "disconnected", "locked"].includes(
          await boat.getAttribute("data-boat-state"),
        ),
      10_000,
    )
    if (!read) {
      failures.push("the Boat account status did not finish reading")
      return
    }
    if ((await boat.locator("input, select").count()) !== 0)
      failures.push("the Boat row shows a field instead of one button")
    if (!/Not connected|Connected|Locked/.test(await boat.innerText()))
      failures.push(`the Boat row says ${JSON.stringify(await boat.innerText())}`)
    if ((await boat.locator(css.boatRuntimeSettings).count()) !== 0)
      failures.push("Auto-stop is inside the Boat row, not a row of its own")
    const connect = boat.locator('[data-boat-action="connect"]')
    if (await connect.count()) {
      await connect.click()
      const dialog = page.locator("[data-boat-dialog]")
      const asked = await dialog
        .locator('input[type="password"]')
        .waitFor({ timeout: 5_000 })
        .then(
          () => true,
          () => false,
        )
      if (!asked) failures.push("Connect… on the Boat row opens no token field")
      const font = await dialog
        .locator('input[type="password"]')
        .evaluate((input) => getComputedStyle(input).fontFamily)
        .catch(() => "")
      if (/mono/i.test(font)) failures.push(`the Boat token field is in ${font}`)
      await page.keyboard.press("Escape")
      await dialog.waitFor({ state: "hidden", timeout: 5_000 })
    }
    const global = page.locator(css.boatRuntimeSettings)
    await global.locator('[data-value="14400"]').click()
    const defaultSaved = await waitFor(
      async () =>
        (await client.boat.runtimeSettings()).defaultLifetime.seconds === 14_400,
      10_000,
    )
    if (!defaultSaved)
      failures.push("the global VM lifetime was not saved through settings")
    receipts.default = await client.boat.runtimeSettings()
    // A choice saves as it is made; the choices are live again once it lands.
    await waitFor(() => global.locator('[data-value="never"]').isEnabled(), 10_000)
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
    await control.waitFor({ timeout: 10_000 })
    // Every control fits inside the dialog, and no setting's name is
    // squeezed onto a line per word beside its control (UI audit #1).
    const fit = await dialog.evaluate((box) => {
      const edge = box.getBoundingClientRect()
      const spilled = [...box.querySelectorAll("button, select, input")]
        .filter((each) => {
          const rect = each.getBoundingClientRect()
          return (
            rect.width > 0 &&
            (rect.right > edge.right + 0.5 || rect.left < edge.left - 0.5)
          )
        })
        .map((each) => each.textContent?.trim() || each.getAttribute("aria-label"))
      const squeezed = [...box.querySelectorAll('[data-slot="settings-row-label"]')]
        .filter((label) => {
          const lines =
            label.getBoundingClientRect().height /
            parseFloat(getComputedStyle(label).lineHeight)
          return lines > 1.5
        })
        .map((label) => label.textContent)
      return { spilled, squeezed, selects: box.querySelectorAll("select").length }
    })
    if (fit.spilled.length > 0)
      failures.push(
        `channel settings controls run past the dialog: ${fit.spilled.join(", ")}`,
      )
    if (fit.squeezed.length > 0)
      failures.push(`channel settings names wrap: ${fit.squeezed.join(", ")}`)
    if (fit.selects > 0) failures.push("channel settings still use a native select")
    const chosen = () => control.getAttribute("data-selection")
    const choose = (value) => control.locator(`[data-value="${value}"]`).click()
    await waitFor(async () => (await chosen()) === "default", 10_000)
    if ((await chosen()) !== "default")
      failures.push("a new channel did not inherit the VM lifetime default")
    await choose("never")
    if (
      !(await waitFor(
        async () =>
          (await client.boat.runtimeSettings({ channelId: channel.channelId }))
            .channelOverride?.mode === "never",
        10_000,
      ))
    )
      failures.push("the channel's explicit unlimited lifetime was not saved")
    await waitFor(() => control.locator('[data-value="default"]').isEnabled(), 10_000)
    receipts.override = await client.boat.runtimeSettings({
      channelId: channel.channelId,
    })
    if (receipts.override.defaultLifetime.seconds !== 14_400)
      failures.push("a channel override changed the global default")
    if (run.evidence)
      await page.screenshot({
        path: join(run.evidence, `boat-channel-${channel.channelId}.png`),
      })
    await choose("default")
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
 * Whether this run's gateway has a connected Boat. A run without Boat keys,
 * or one whose gateway cannot answer, has nothing to place agents on, so the
 * checks that need Boat skip instead of failing. That Boat is offered only
 * once connected is held by channel-settings.test.tsx.
 */
export async function boatProbe(client) {
  const state = await client.boat.status().then(
    (status) => status?.state ?? "unknown",
    (error) => `unreadable (${error?.refusal ?? error?.code ?? error?.message ?? error})`,
  )
  return { ready: state === "connected", state }
}

/**
 * Changes where a made channel runs from its settings dialog, as a person
 * does, after it has been talked to. New threads move first: the main chat
 * stays the same one. Then the main agent moves: the channel names a fresh
 * main chat and the one already talked to stays in the channel as a thread.
 * Both go back to this machine at the end, which names yet another fresh main
 * chat, so the channel is left running where it started. Nothing is sent
 * while it is on Boat.
 *
 * It runs only when `boatProbe` finds Boat connected, and says so when it
 * skips.
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
  const boat = await boatProbe(client)
  receipts.boat = boat
  if (!boat.ready) {
    log(
      `channel ${channel.channelId}: Boat is ${boat.state}, so its run-location checks are skipped`,
    )
    if (run.evidence)
      writeFileSync(
        join(run.evidence, `channel-placement-${channel.channelId}.json`),
        JSON.stringify(receipts, null, 2) + "\n",
      )
    return
  }
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
  // Each option is found by its button's label. A role query for that
  // exact name missed Boat in a stress run while the dialog already showed it.
  const option = (row, name) =>
    control
      .locator(`[data-placement-row="${row}"]`)
      .locator("button")
      .filter({ hasText: new RegExp(`^${name}$`) })
  // A choice is done when the dialog shows it, not when it stops being busy:
  // a save not started yet is not busy either.
  const shows = (row, place) =>
    waitFor(async () => (await control.getAttribute(attribute[row])) === place, 10_000)
  await control.waitFor({ timeout: 10_000 })
  // Boat is connected, so the dialog offers it once it has read that.
  const offered = await waitFor(
    async () =>
      (await option("threads", "Boat").count()) > 0 &&
      !(await option("threads", "Boat").isDisabled()),
    10_000,
  )
  const toBoat = async (row) => {
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
    receipts.boatOffered = Boolean(offered)
    if (!offered) {
      failures.push("Boat is connected but the channel settings do not offer it")
      return
    }
    await toBoat("threads")
    const threads = await settled({
      coordinatorPlacement: before.coordinatorPlacement,
      threadPlacement: "boat",
    })
    receipts.threads = threads
    if (!threads) failures.push("moving new threads to Boat was not saved")
    else if (threads.coordinatorConversationId !== before.coordinatorConversationId)
      failures.push("moving only new threads named a new main chat")
    await toBoat("coordinator")
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
