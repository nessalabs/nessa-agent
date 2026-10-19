#!/usr/bin/env node
/**
 * The owner's share dialog (nessa_ui#128, #129) against a scripted product
 * session: Share… on a session's menu, its grants and what each holds, a
 * Drive grant with a preset and hosts sent as the wire takes it, the host
 * list held to the hosts' own bound however full the tool patterns are, a
 * refusal said in words,
 * a revoke, a change refused because another window changed the grant and
 * started again from what it holds, a list held while it predates this
 * window's own change, focus kept in the dialog and given back,
 * and the panel within the window. Chromium and WebKit.
 *
 * The page is the dev server's, opened with `?gateway` so it uses the browser
 * session. `/browser/check` is answered here, and so is the product socket:
 * no gateway binary. Rows D1, D4–D6, D9 to D11 of
 * `docs/design/read-grants.md` › "Dialog state and order".
 */
import { mkdirSync, readFileSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { openPage, need, withEngines } from "./lib/browser.mjs"
import { attempt } from "./lib/cli.mjs"
import { main } from "./lib/run.mjs"
import { css, names, selectorFor } from "./lib/selectors.mjs"

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../../..")
const fixtures = JSON.parse(
  readFileSync(join(repo, "protocol/product/fixtures.json"), "utf8"),
)

const wireCodes = [
  "share_target_not_paired",
  "share_grant_changed",
  "this-session",
  "another-session",
  "invalid_request",
  "conversation_not_found",
  "drive_policy_unenforceable",
  "temporarily_unavailable",
  "forbidden",
  "unauthorized",
  "read_only_tools",
  "no_shell",
  "not-paired",
  "unanswered",
]

const conversationId = "00000000-0000-4000-8000-0000000000a1"
const title = "Release notes"
const grant = (action) => ({
  action,
  resource: { organizationId: "personal", id: "gateway" },
})

const sessionReady = {
  version: 1,
  gatewayId: "gateway",
  principalId: "owner",
  organizationId: "personal",
  membershipId: "membership",
  credentialId: "owner-credential",
  audienceId: "gateway",
  expiresAt: 2_000_000_000,
  grants: [
    grant("conversation.read"),
    grant("conversation.write"),
    grant("credential.manage"),
  ],
  methods: [
    "server.health",
    "auth.session",
    "agents.list",
    "conversation.subscribeList",
    "conversation.unsubscribe",
    "conversation.shares",
    "conversation.share",
    "conversation.unshare",
    "credential.list",
  ],
}

const listed = {
  conversationId,
  title,
  preview: "Ready for review",
  createdAtMs: 1_700_000_000_000,
  updatedAtMs: 1_700_000_100_000,
  running: false,
  archived: false,
}

function credential(id, actions, issuedAt = 1_700_000_000) {
  return {
    id,
    principalId: "owner",
    organizationId: "personal",
    audienceId: "gateway",
    issuedAt,
    expiresAt: null,
    revokedAt: null,
    grants: actions.map(grant),
  }
}

const devices = [
  credential("owner-credential", ["conversation.read", "credential.manage"]),
  credential("device-phone-0001", ["conversation.read"], 1_700_000_000),
  credential("device-tablet-0002", ["conversation.read"], 1_710_000_000),
]

const protocol = JSON.parse(readFileSync(join(repo, "protocol/product/v1.json"), "utf8"))
const sharePageSize = protocol.$defs.ConversationSharesParams.properties.pageSize.maximum
const hosts = ["devbox", "me@build.example", "ci-runner", "staging"]

/**
 * A product socket that lists one conversation and answers sharing.
 * `scenario.refuseShare` refuses every share with that code;
 * `scenario.holdShare` answers each share only once `release()` is called;
 * after `holdReads()`, each `conversation.shares` read waits for it too.
 * A shares read names a page size and is answered with one page: a cursor
 * only when further grants remain, naming that page's last grant.
 * As the gateway does, a policy revision comes from one count that only goes
 * up, a share answers the grant's revision, and a share or unshare naming an
 * `expectedPolicyRevision` the grant no longer holds is refused
 * `share_grant_changed`, changing nothing. `changeElsewhere` is another
 * window's share, which this window has not read.
 * `sent` keeps each share and unshare's params; `unexpected` names any method this check
 * does not answer, so a new call does not pass by being ignored.
 */
function scriptedGateway(scenario = {}) {
  let revision = 2
  let catalogue = 1
  let grants = scenario.grants ?? [
    {
      credentialId: "device-phone-0001",
      capabilities: ["read", "comment", "drive"],
      policy: { preset: "read_only_tools", commandHosts: ["devbox"] },
      policyRevision: revision,
      grantedAtMs: 1_700_000_000_000,
    },
  ]
  let subscriptions = 0
  const sent = []
  const unexpected = []
  const held = []
  let holdingReads = false
  let failingReads = false

  /** The grant `params` names, or a refusal when it is not at the revision expected. */
  function expected(params) {
    const before = grants.find((each) => each.credentialId === params.credentialId)
    // As the gateway: 0 names no grant held, since a revision starts at 1.
    const stale =
      params.expectedPolicyRevision !== undefined &&
      (before?.policyRevision ?? 0) !== params.expectedPolicyRevision
    return { before, stale }
  }
  const changed = {
    ok: false,
    error: { code: "share_grant_changed", message: "changed" },
  }

  function share(params) {
    const { before, stale } = expected(params)
    if (stale) return changed
    const same =
      before &&
      JSON.stringify([...(before.capabilities ?? [])].sort()) ===
        JSON.stringify([...(params.capabilities ?? [])].sort()) &&
      JSON.stringify(before.policy) === JSON.stringify(params.policy)
    if (same)
      return {
        ok: true,
        payload: {
          requestId: params.requestId,
          applied: false,
          policyRevision: before.policyRevision,
        },
      }
    revision += 1
    // A new grant moves the catalogue. A change of capabilities or policy
    // does not, as the gateway's change_revision does not.
    if (!before) catalogue += 1
    const next = {
      credentialId: params.credentialId,
      capabilities: params.capabilities,
      ...(params.policy ? { policy: params.policy } : {}),
      policyRevision: revision,
      grantedAtMs: before?.grantedAtMs ?? 1_760_000_000_000,
    }
    grants = before
      ? grants.map((each) => (each === before ? next : each))
      : [...grants, next]
    return {
      ok: true,
      payload: { requestId: params.requestId, applied: true, policyRevision: revision },
    }
  }

  /** One page of grants, oldest first, as `conversation.shares` answers. */
  function sharesPage(params) {
    const pageSize = params?.pageSize
    if (!Number.isInteger(pageSize) || pageSize < 1 || pageSize > sharePageSize)
      return { ok: false, error: { code: "invalid_request", message: "page" } }
    let start = 0
    if (params.cursor) {
      const at = grants.findIndex(
        (each) =>
          each.grantedAtMs === params.cursor.grantedAtMs &&
          each.credentialId === params.cursor.credentialId,
      )
      start = at < 0 ? grants.length : at + 1
    }
    const items = grants.slice(start, start + pageSize)
    const last = items[items.length - 1]
    const more = start + items.length < grants.length
    return {
      ok: true,
      payload: {
        items,
        changeRevision: catalogue,
        ...(more && last
          ? { cursor: { grantedAtMs: last.grantedAtMs, credentialId: last.credentialId } }
          : {}),
      },
    }
  }

  function reply(method, params) {
    switch (method) {
      case "server.health":
        return { ok: true, payload: { ok: true, runtimeStatus: "ready", uptimeMs: 1 } }
      case "auth.session":
        return { ok: true, payload: sessionReady }
      case "conversation.subscribeList":
        subscriptions += 1
        return { ok: true, payload: { subscriptionId: String(subscriptions) } }
      case "conversation.unsubscribe":
        return { ok: true, payload: {} }
      // The window follows the session it selects; its transcript is not
      // this check's, so it is not read.
      case "conversation.subscribe":
        return {
          ok: false,
          error: { code: "temporarily_unavailable", message: "not read" },
        }
      case "credential.list":
        return { ok: true, payload: { credentials: scenario.devices ?? devices } }
      case "pairing.pending":
        // Linked devices, once the empty dialog's link has opened it.
        return { ok: true, payload: { items: [] } }
      case "agents.list":
        return {
          ok: true,
          payload: {
            ...fixtures.AgentsListResult,
            environments: ["here", ...hosts],
            commandHosts: hosts,
          },
        }
      case "conversation.shares": {
        if (failingReads)
          return {
            ok: false,
            error: { code: "temporarily_unavailable", message: "busy" },
          }
        const answer = () => sharesPage(params)
        if (!holdingReads) return answer()
        return new Promise((resolve) => held.push(() => resolve(answer())))
      }
      case "conversation.share": {
        sent.push({ method, params })
        if (scenario.refuseShare)
          return { ok: false, error: { code: scenario.refuseShare, message: "refused" } }
        if (!scenario.holdShare) return share(params)
        return new Promise((resolve) => held.push(() => resolve(share(params))))
      }
      case "conversation.unshare": {
        sent.push({ method, params })
        const { before, stale } = expected(params)
        if (stale) return changed
        if (before) catalogue += 1
        grants = grants.filter((each) => each !== before)
        return {
          ok: true,
          payload: { requestId: params.requestId, applied: before !== undefined },
        }
      }
      default:
        unexpected.push(method)
        return {
          ok: false,
          error: { code: "invalid_request", message: "not this check" },
        }
    }
  }

  function route(socket) {
    const nonce = `share-${Math.random().toString(16).slice(2)}`
    // Events continue the socket's one sequence, after the challenge's.
    let seq = 1
    socket.send(
      JSON.stringify({
        type: "event",
        event: "session.challenge",
        seq: 1,
        stateVersion: 0,
        payload: { minVersion: 1, maxVersion: 1, nonce, expiresAt: 2_000_000_000 },
      }),
    )
    socket.onMessage(async (raw) => {
      let frame
      try {
        frame = JSON.parse(String(raw))
      } catch {
        return
      }
      if (!frame || typeof frame.id !== "string") return
      if (frame.method === "session.authenticate") {
        socket.send(
          JSON.stringify(
            frame.params?.nonce === nonce
              ? { type: "res", id: frame.id, ok: true, payload: sessionReady }
              : {
                  type: "res",
                  id: frame.id,
                  ok: false,
                  error: { code: "unauthorized", message: "nonce" },
                },
          ),
        )
        return
      }
      const result = await reply(frame.method, frame.params)
      socket.send(JSON.stringify({ type: "res", id: frame.id, ...result }))
      if (frame.method === "conversation.subscribeList" && result.ok) {
        seq += 1
        socket.send(
          JSON.stringify({
            type: "event",
            event: "conversation.listed",
            seq,
            stateVersion: 0,
            payload: {
              subscriptionId: result.payload.subscriptionId,
              list: { conversations: [listed], complete: true },
            },
          }),
        )
      }
    })
  }

  /** Answers every call held so far. */
  const release = () => held.splice(0).forEach((answer) => answer())
  const holdReads = () => {
    holdingReads = true
  }
  /** While on, each `conversation.shares` read is refused, as a busy gateway would. */
  const failReads = (on) => {
    failingReads = on
  }
  /** Another window's share, unseen by this one until it reads again. */
  const changeElsewhere = (params) => share({ ...params, requestId: "elsewhere" })
  return { route, sent, unexpected, release, holdReads, failReads, changeElsewhere }
}

function gatewayUrl(url) {
  const page = new URL(url)
  page.searchParams.set("gateway", "1")
  return page.href
}

async function openGateway(browser, url, scenario) {
  const gate = scriptedGateway(scenario)
  const opened = await openPage(browser, {
    url: gatewayUrl(url),
    layout: "columns",
    width: 1200,
    height: 900,
    beforeLoad: async (context) => {
      const ok = (route) =>
        route.fulfill({ status: 200, contentType: "application/json", body: "{}" })
      await context.route("**/browser/check", ok)
      await context.route("**/browser/login", ok)
      await context.routeWebSocket(/\/browser\/session/, gate.route)
    },
  })
  return { ...opened, gate }
}

/** Share… from the session's menu, as a person does, until the grants are read. */
async function openShare(page) {
  const row = page.locator(selectorFor.sessionListRowFor(conversationId))
  await row.waitFor({ timeout: 15_000 })
  await row.click({ button: "right" })
  await need(page, css.menu, "the session's menu")
  await page.locator(css.menuItem, { hasText: names.share }).click()
  await page.waitForSelector(`${css.shareDialog}[data-share-state="read"]`, {
    timeout: 10_000,
  })
}

async function settled(page) {
  await page.waitForSelector(`${css.shareDialog}[data-share-state="read"]`, {
    timeout: 10_000,
  })
}

/** Whether the form, Share and the first Revoke are each held (disabled). */
async function heldState(page) {
  return {
    // Every control in the form, not the fieldset's own flag: each must refuse input.
    form: await page
      .locator(css.shareForm)
      .evaluate((form) =>
        [...form.querySelectorAll("input, button")].every((each) =>
          each.matches(":disabled"),
        ),
      ),
    submit: await page.locator(selectorFor.shareAction("submit")).isDisabled(),
    revoke: await page.locator(selectorFor.shareAction("revoke")).first().isDisabled(),
  }
}

async function dialogText(page) {
  return page.locator(css.shareDialog).innerText()
}

const codesIn = (text) => wireCodes.filter((code) => text.includes(code))

/** Chooses a kit card or segmented option inside `group` by its visible text. */
async function choose(page, group, text) {
  await page.locator(`${group} label, ${group} button`, { hasText: text }).first().click()
}

async function shot(page, options, engine, name) {
  if (!options.shots) return null
  mkdirSync(options.shots, { recursive: true })
  const path = join(options.shots, `${engine}-${name}.png`)
  await page.locator(css.shareDialog).screenshot({ path })
  return path
}

const meta = {
  name: "share-dialog",
  summary:
    "Share dialog: grants, Drive policy and hosts, the host budget, refusal, revoke, focus, fit",
  defaults: { engine: "chromium,webkit" },
  help: `
Usage: node verification/desktop/scripts/share-dialog.mjs [options] [--shots <dir>]

Per engine, against a scripted product session:
  open      Share… on a session's menu opens one dialog, focus inside it,
            its grant listed with role, tools and hosts in words
  drive     a Drive grant with a preset and two hosts is sent as the wire
            takes it, naming revision 0 as a first grant, said as done, and
            listed as the gateway read it back
  empty     with no linked device and nothing shared, one line and Link a
            Device, which closes the dialog and opens Settings on Linked
            devices; no Done, no Share
  budget    both pattern lists full still leave every host: "4 of 16", and
            Share can send
  refusal   a refused share is said in words; no wire code on the page
  held      while a share is on its way, and the read after it, the form,
            Share and Revoke are held; once that read answers they take input
  revoke    Revoke asks first, then sends one unshare naming the revision it
            read; the row goes when the list is read
  changed   a change to a grant another window changed since it was read
            names the revision it was built on and is refused: said in
            words, Share held as changed, Start Again starts from what it
            holds, and the next share names that revision and is made
  stale     the read after a change fails: the list, the form, Share,
            Change and Revoke are held, no reason and no Start Again, and
            Try Again reads it, with no reason after
  focus     Tab stays in the dialog; Escape closes it and focus comes back
            to the session's row
  fit-N     at 1200 and 420 wide the panel is inside the window and the page does not scroll sideways`,
}

await main(meta, async ({ options, rep, url }) => {
  await withEngines(options, rep, async (engine, browser) => {
    async function check(name, scenario, body, size) {
      await attempt(rep, { engine, name }, async () => {
        const opened = await openGateway(browser, url, scenario)
        try {
          await openShare(opened.page)
          // Opened where the session's row is shown, then narrowed around it.
          if (size) await opened.page.setViewportSize(size)
          const detail = await body(opened.page, opened.gate)
          const text = await dialogText(opened.page).catch(() => "")
          return {
            ...detail,
            failures: [
              ...detail.failures,
              ...codesIn(text).map((code) => `wire code on the page: ${code}`),
              ...opened.gate.unexpected.map((method) => `unexpected method ${method}`),
            ],
          }
        } finally {
          await opened.close()
        }
      })
    }

    await check("open", {}, async (page) => {
      const focusInside = await page.evaluate(
        (selector) => document.querySelector(selector)?.contains(document.activeElement),
        css.shareDialog,
      )
      const heading = await page.locator(`${css.shareDialog} h2`).first().innerText()
      const row = await page
        .locator(selectorFor.shareGrant("device-phone-0001"))
        .innerText()
      const evidence = await shot(page, options, engine, "grants")
      return {
        heading,
        row,
        evidence,
        failures: [
          focusInside ? null : "focus was not inside the dialog",
          heading.includes(title) ? null : `the title was ${JSON.stringify(heading)}`,
          row.includes("Drive · Read-only tools, commands on devbox")
            ? null
            : `the grant read ${JSON.stringify(row)}`,
          (await page.locator(css.shareDialog).count()) === 1 ? null : "not one dialog",
        ].filter(Boolean),
      }
    })

    await check("empty", { devices: [], grants: [] }, async (page) => {
      await need(page, css.shareEmpty, "the empty line")
      const said = await page.locator(css.shareEmpty).innerText()
      const evidence = await shot(page, options, engine, "empty")
      const buttons = await page.locator(`${css.shareDialog} button`).allInnerTexts()
      await page.locator(css.shareLinkDevice).click()
      const settings = await page
        .locator(css.settingsLinkedDevices)
        .first()
        .waitFor({ timeout: 5_000 })
        .then(() => true)
        .catch(() => false)
      return {
        said,
        buttons,
        evidence,
        failures: [
          said.replace(/\s+/g, " ").trim() === "No linked devices yet. Link a Device"
            ? null
            : `the empty dialog said ${JSON.stringify(said)}`,
          // The × and the link, nothing else: no Done, no Share held back.
          buttons.filter((each) => each.trim() !== "").join("|") === "Link a Device"
            ? null
            : `its buttons were ${JSON.stringify(buttons)}`,
          (await page.locator(css.shareDialog).count()) === 0
            ? null
            : "the link left the dialog open",
          settings ? null : "Settings did not open on Linked devices",
        ].filter(Boolean),
      }
    })

    await check("drive", {}, async (page, gate) => {
      await choose(page, css.shareTargets, "0002")
      await choose(page, css.shareRole, "Drive")
      await need(page, css.shareNotes, "the Drive notes")
      const notes = await page.locator(css.shareNotes).innerText()
      await choose(page, css.sharePresets, "No shell")
      const boxes = page.locator(`${css.shareHosts} input[type="checkbox"]`)
      await boxes.nth(0).check()
      await boxes.nth(2).check()
      const count = await page.locator(css.shareHostCount).innerText()
      const evidence = await shot(page, options, engine, "drive")
      await page.locator(selectorFor.shareAction("submit")).click()
      await page.waitForFunction(
        (selector) => document.querySelector(selector)?.textContent === "Shared.",
        css.shareNotice,
        { timeout: 10_000 },
      )
      await settled(page)
      const share = gate.sent.find((each) => each.method === "conversation.share")
      const row = await page
        .locator(selectorFor.shareGrant("device-tablet-0002"))
        .innerText()
      const policy = JSON.stringify(share?.params?.policy)
      return {
        policy,
        count,
        evidence,
        failures: [
          gate.sent.length === 1 ? null : `${gate.sent.length} changes were sent`,
          share?.params?.conversationId === conversationId
            ? null
            : "another conversation",
          JSON.stringify([...(share?.params?.capabilities ?? [])].sort()) ===
          JSON.stringify(["comment", "drive", "read"])
            ? null
            : `capabilities ${JSON.stringify(share?.params?.capabilities)}`,
          policy ===
          JSON.stringify({ preset: "no_shell", commandHosts: ["devbox", "ci-runner"] })
            ? null
            : `policy ${policy}`,
          typeof share?.params?.requestId === "string" ? null : "no request id",
          // A first grant names 0: no grant held, so another window's first
          // grant landing before it is refused rather than overwritten.
          share?.params?.expectedPolicyRevision === 0
            ? null
            : `a first grant expected ${share?.params?.expectedPolicyRevision}`,
          count.includes("2 of 16") ? null : `the count read ${JSON.stringify(count)}`,
          notes.includes("sandbox")
            ? null
            : "the note on what a policy cannot stop was missing",
          row.includes("Drive · No shell, commands on devbox and ci-runner")
            ? null
            : `the new grant read ${JSON.stringify(row)}`,
        ].filter(Boolean),
      }
    })

    await check("budget", {}, async (page, gate) => {
      await choose(page, css.shareTargets, "0002")
      await choose(page, css.shareRole, "Drive")
      await choose(page, css.sharePresets, "Custom")
      const many = (prefix, count) =>
        Array.from({ length: count }, (_, index) => `${prefix}${index}`).join(", ")
      await page.locator(selectorFor.sharePattern("allow")).fill(many("Allowed", 16))
      await page.locator(selectorFor.sharePattern("deny")).fill(many("Denied", 16))
      const boxes = page.locator(`${css.shareHosts} input[type="checkbox"]`)
      for (const index of [0, 1, 2, 3]) await boxes.nth(index).check()
      const count = await page.locator(css.shareHostCount).innerText()
      const canSend = await page.locator(selectorFor.shareAction("submit")).isEnabled()
      const evidence = await shot(page, options, engine, "budget")
      return {
        count,
        evidence,
        failures: [
          count.includes("4 of 16 selected")
            ? null
            : `the count read ${JSON.stringify(count)}`,
          canSend ? null : "Share was held with a full pattern list and every host",
          gate.sent.length === 0 ? null : "a change was sent",
        ].filter(Boolean),
      }
    })

    await check(
      "refusal",
      { refuseShare: "share_target_not_paired" },
      async (page, gate) => {
        await choose(page, css.shareTargets, "0002")
        await page.locator(selectorFor.shareAction("submit")).click()
        await page.waitForFunction(
          (selector) => (document.querySelector(selector)?.textContent ?? "").length > 0,
          css.shareNotice,
          { timeout: 10_000 },
        )
        await settled(page)
        const notice = await page.locator(css.shareNotice).innerText()
        const evidence = await shot(page, options, engine, "refusal")
        return {
          notice,
          evidence,
          failures: [
            notice.includes("no longer linked to you")
              ? null
              : `the notice read ${notice}`,
            gate.sent.length === 1 ? null : `${gate.sent.length} changes were sent`,
          ].filter(Boolean),
        }
      },
    )

    await check("held", { holdShare: true }, async (page, gate) => {
      await choose(page, css.shareTargets, "0002")
      gate.holdReads()
      await page.locator(selectorFor.shareAction("submit")).click()
      await page.waitForSelector(`${css.shareDialog}[data-share-state="held"]`, {
        timeout: 10_000,
      })
      const whileSending = await heldState(page)
      const reasonWhileSending = await page
        .locator(css.shareReason)
        .getAttribute("data-share-reason")
      // The share is answered; the read after it is not, yet.
      gate.release()
      await page.waitForFunction(
        (selector) => document.querySelector(selector)?.textContent === "Shared.",
        css.shareNotice,
        { timeout: 10_000 },
      )
      const whileReading = await heldState(page)
      // The draft is on the share's answer and the list predates it: nothing
      // is said as changed under it until the list is read again.
      const reasonsWhileReading = await page.locator(css.shareReason).count()
      const stateWhileReading = await page
        .locator(css.shareDialog)
        .getAttribute("data-share-state")
      gate.release()
      await settled(page)
      const after = await heldState(page)
      const heldAll = (state) => state.form && state.submit && state.revoke
      return {
        whileSending,
        whileReading,
        after,
        failures: [
          heldAll(whileSending)
            ? null
            : `not held while sending: ${JSON.stringify(whileSending)}`,
          reasonWhileSending === "this-session"
            ? null
            : `the wait was said as ${reasonWhileSending}`,
          heldAll(whileReading) && stateWhileReading === "held"
            ? null
            : `not held while the read after it was on its way: ${JSON.stringify(whileReading)} ${stateWhileReading}`,
          reasonsWhileReading === 0
            ? null
            : "a reason was shown while the read after the share was on its way",
          after.form || after.submit || after.revoke
            ? `still held once read again: ${JSON.stringify(after)}`
            : null,
          gate.sent.length === 1 ? null : `${gate.sent.length} changes were sent`,
        ].filter(Boolean),
      }
    })

    await check("revoke", {}, async (page, gate) => {
      const row = selectorFor.shareGrant("device-phone-0001")
      await page.locator(`${row} ${selectorFor.shareAction("revoke")}`).click()
      const asked = await page.locator(row).innerText()
      const sentBefore = gate.sent.length
      await page.locator(`${row} ${selectorFor.shareAction("confirm-revoke")}`).click()
      await page.waitForSelector(selectorFor.shareGrant("device-phone-0001"), {
        state: "detached",
        timeout: 10_000,
      })
      const notice = await page.locator(css.shareNotice).innerText()
      const unshare = gate.sent.find((each) => each.method === "conversation.unshare")
      return {
        failures: [
          asked.includes("Revoke this device") ? null : "revoke was not asked first",
          sentBefore === 0 ? null : "a change was sent before it was confirmed",
          gate.sent.length === 1 ? null : `${gate.sent.length} changes were sent`,
          unshare?.params?.credentialId === "device-phone-0001"
            ? null
            : `unshare named ${unshare?.params?.credentialId}`,
          unshare?.params?.expectedPolicyRevision === 2
            ? null
            : `unshare expected revision ${unshare?.params?.expectedPolicyRevision}`,
          notice === "No longer shared." ? null : `the notice read ${notice}`,
        ].filter(Boolean),
      }
    })

    await check("changed", {}, async (page, gate) => {
      const row = selectorFor.shareGrant("device-phone-0001")
      await page.locator(`${row} ${selectorFor.shareAction("change")}`).click()
      await choose(page, css.sharePresets, "No shell")
      // Another window changes the grant after this one read it.
      gate.changeElsewhere({
        credentialId: "device-phone-0001",
        capabilities: ["read", "comment"],
      })
      await page.locator(selectorFor.shareAction("submit")).click()
      await page.waitForSelector(`${css.shareNotice}[data-share-notice="changed"]`, {
        timeout: 10_000,
      })
      await settled(page)
      const notice = await page.locator(css.shareNotice).innerText()
      const reason = await page.locator(css.shareReason).getAttribute("data-share-reason")
      const heldAsChanged = await page
        .locator(selectorFor.shareAction("submit"))
        .isDisabled()
      const listed = await page.locator(row).innerText()
      const evidence = await shot(page, options, engine, "changed")
      await page.locator(selectorFor.shareAction("restart")).click()
      const restarted = {
        reason: await page.locator(css.shareReason).count(),
        role: await page
          .locator(`${css.shareRole} [aria-pressed="true"]`)
          .innerText()
          .catch(() => null),
      }
      await choose(page, css.shareRole, "Read")
      await page.locator(selectorFor.shareAction("submit")).click()
      await page.waitForFunction(
        (selector) => document.querySelector(selector)?.textContent === "Shared.",
        css.shareNotice,
        { timeout: 10_000 },
      )
      await settled(page)
      const after = await page.locator(row).innerText()
      const [refused, made] = gate.sent
      return {
        notice,
        reason,
        restarted,
        evidence,
        failures: [
          refused?.params?.expectedPolicyRevision === 2
            ? null
            : `the refused share expected ${refused?.params?.expectedPolicyRevision}`,
          notice.includes("changed since this list was read")
            ? null
            : `the notice read ${JSON.stringify(notice)}`,
          reason === "changed" ? null : `the reason was ${reason}`,
          heldAsChanged ? null : "Share could send a draft changed under it",
          listed.includes("Comment") ? null : `the grant read ${JSON.stringify(listed)}`,
          restarted.reason === 0 ? null : "still held as changed after Start Again",
          restarted.role === "Comment"
            ? null
            : `Start Again began from ${restarted.role}`,
          made?.params?.expectedPolicyRevision === 3
            ? null
            : `the next share expected ${made?.params?.expectedPolicyRevision}`,
          after.includes("Read") && !after.includes("Comment")
            ? null
            : `the grant read ${JSON.stringify(after)}`,
          gate.sent.length === 2 ? null : `${gate.sent.length} changes were sent`,
        ].filter(Boolean),
      }
    })

    await check("stale", {}, async (page, gate) => {
      const row = selectorFor.shareGrant("device-phone-0001")
      await page.locator(`${row} ${selectorFor.shareAction("change")}`).click()
      await choose(page, css.shareRole, "Comment")
      // The read after this window's own change fails: the list predates it.
      gate.failReads(true)
      await page.locator(selectorFor.shareAction("submit")).click()
      await page.waitForSelector(
        `${css.shareReadFailure}[data-share-read-failure="grants"]`,
        {
          timeout: 10_000,
        },
      )
      const stale = {
        state: await page.locator(css.shareDialog).getAttribute("data-share-state"),
        notice: await page.locator(css.shareNotice).getAttribute("data-share-notice"),
        reasons: await page.locator(css.shareReason).count(),
        restart: await page.locator(selectorFor.shareAction("restart")).count(),
        ...(await heldState(page)),
        change: await page
          .locator(`${row} ${selectorFor.shareAction("change")}`)
          .isDisabled(),
      }
      gate.failReads(false)
      await page.locator(selectorFor.shareAction("retry")).click()
      await page.waitForSelector(css.shareReadFailure, {
        state: "detached",
        timeout: 10_000,
      })
      await settled(page)
      const after = {
        state: await page.locator(css.shareDialog).getAttribute("data-share-state"),
        reasons: await page.locator(css.shareReason).count(),
        row: await page.locator(row).innerText(),
      }
      return {
        stale,
        after,
        failures: [
          stale.state === "held" ? null : `the stale list read as ${stale.state}`,
          stale.notice === "done" ? null : `the change settled ${stale.notice}`,
          stale.form && stale.submit && stale.revoke && stale.change
            ? null
            : `a stale list could be acted on: ${JSON.stringify(stale)}`,
          stale.reasons === 0 ? null : "a reason was said beside a stale list",
          stale.restart === 0 ? null : "Start Again was offered on a stale list",
          after.state === "read" ? null : `after Try Again it read as ${after.state}`,
          after.reasons === 0 ? null : "a reason was said once read again",
          after.row.includes("Comment")
            ? null
            : `the grant read ${JSON.stringify(after.row)}`,
          gate.sent.length === 1 ? null : `${gate.sent.length} changes were sent`,
        ].filter(Boolean),
      }
    })

    await check("focus", {}, async (page) => {
      const trail = []
      for (let index = 0; index < 40; index++) {
        await page.keyboard.press("Tab")
        trail.push(
          await page.evaluate(
            (selector) =>
              document.querySelector(selector)?.contains(document.activeElement),
            css.shareDialog,
          ),
        )
      }
      await page.keyboard.press("Escape")
      await page.waitForSelector(css.shareDialog, { state: "detached", timeout: 5_000 })
      // The end state, not a proxy for it: focus on the row, however long the
      // dialog's close takes to give it back.
      await page
        .waitForFunction(
          (id) =>
            document.activeElement?.closest(`[data-session-row="${id}"]`) !== null &&
            document.activeElement !== document.body,
          conversationId,
          { timeout: 3_000 },
        )
        .catch(() => {})
      const back = await page.evaluate((id) => {
        const active = document.activeElement
        return {
          row: active?.closest(`[data-session-row="${id}"]`) !== null,
          what: active ? active.outerHTML.slice(0, 160) : null,
        }
      }, conversationId)
      return {
        back,
        failures: [
          trail.every(Boolean)
            ? null
            : `Tab left the dialog at step ${trail.indexOf(false) + 1}`,
          back.row ? null : `focus came back to ${back.what}, not the session's row`,
        ].filter(Boolean),
      }
    })

    for (const size of [
      { width: 1200, height: 900 },
      { width: 420, height: 700 },
    ])
      await check(
        `fit-${size.width}`,
        {},
        async (page) => {
          await choose(page, css.shareTargets, "0002")
          await choose(page, css.shareRole, "Drive")
          const measured = await page.evaluate((selector) => {
            const panel = document.querySelector(selector)?.getBoundingClientRect()
            const body = document.querySelector(`${selector} [data-slot="dialog-body"]`)
            return {
              panel: panel && {
                left: panel.left,
                top: panel.top,
                right: panel.right,
                bottom: panel.bottom,
              },
              window: { width: innerWidth, height: innerHeight },
              scrolls: body ? body.scrollHeight > body.clientHeight : null,
              pageScrolls: document.documentElement.scrollWidth > innerWidth,
            }
          }, css.shareDialog)
          const evidence =
            size.width < 600 ? await shot(page, options, engine, "narrow") : null
          const { panel, window } = measured
          return {
            size,
            measured,
            evidence,
            failures: [
              panel ? null : "the panel was not measured",
              panel && panel.left >= 0 && panel.right <= window.width
                ? null
                : "the panel is wider than the window",
              panel && panel.top >= 0 && panel.bottom <= window.height
                ? null
                : "the panel is taller than the window",
              measured.pageScrolls ? "the page scrolls sideways" : null,
            ].filter(Boolean),
          }
        },
        size,
      )
  })
})
