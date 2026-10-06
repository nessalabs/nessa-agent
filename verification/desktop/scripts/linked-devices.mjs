#!/usr/bin/env node
/**
 * Settings › Connections › Linked devices (#462) against a scripted product
 * session: linking off, a refused owner, a code with both orbs, approve,
 * revoke, and an expired code. Chromium and WebKit. The phone's scanner is
 * not this screen.
 *
 * The page is the dev server's, opened with `?gateway` so it uses the browser
 * session. `/browser/check` is answered here, and so is the product socket:
 * no gateway binary. Rows L1–L3, L5, L7, L11, L15, L17, L18 and L20.
 */
import { openPage, need, withEngines } from "./lib/browser.mjs"
import { attempt } from "./lib/cli.mjs"
import { main } from "./lib/run.mjs"
import { css, keys, names } from "./lib/selectors.mjs"

const wireCodes = [
  "pairing_not_configured",
  "pairing_not_found",
  "pairing_busy",
  "pairing_unavailable",
  "pairing_slot_occupied",
  "pairing_capacity",
  "pairing_conflict",
  "pairing_ineligible",
  "forbidden",
  "unauthorized",
  "invalid_request",
  "temporarily_unavailable",
  "credential_not_found",
  "credential_conflict",
  "credential_capacity",
]

const invitationId = Array.from({ length: 16 }, (_, index) => index + 1)
const deviceKey = Array.from({ length: 32 }, () => 1)
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
  grants: [grant("conversation.read"), grant("credential.manage")],
  methods: [
    "server.health",
    "auth.session",
    "conversation.list",
    "credential.list",
    "credential.revoke",
    "pairing.pending",
    "pairing.create",
    "pairing.cancel",
    "pairing.approve",
  ],
}

function status(patch) {
  return {
    invitationId,
    consentId: Array.from({ length: 16 }, () => 2),
    generation: 1,
    class: "gateway-conversation-read",
    grant: grant("conversation.read"),
    createdAtMs: 1_000,
    expiresAtMs: Date.now() + 120_000,
    phase: "available",
    cleanupPending: false,
    ...patch,
  }
}

function credential(id, actions, revokedAt = null) {
  return {
    id,
    principalId: "owner",
    organizationId: "personal",
    audienceId: "gateway",
    issuedAt: 1_700_000_000,
    expiresAt: null,
    revokedAt,
    grants: actions.map(grant),
  }
}

const same = (a, b) =>
  Array.isArray(a) &&
  Array.isArray(b) &&
  a.length === b.length &&
  a.every((byte, index) => byte === b[index])

/**
 * A product socket that answers the methods this screen calls. `scenario`
 * is the case: linking off, refused, or on, with whatever is already open.
 */
function scriptedGateway(scenario) {
  const invitations = [...(scenario.invitations ?? [])]
  let devices = [...(scenario.devices ?? [])]
  const unexpected = []
  const known = new Set([
    "server.health",
    "conversation.list",
    "auth.session",
    "credential.list",
    "credential.revoke",
    "pairing.pending",
    "pairing.create",
    "pairing.cancel",
    "pairing.status",
    "pairing.approve",
    "pairing.deny",
  ])

  function reply(method, params) {
    if (!known.has(method)) {
      unexpected.push(method)
      return { ok: false, error: { code: "invalid_request", message: "not this check" } }
    }
    if (method === "server.health")
      return {
        ok: true,
        payload: { ok: true, runtimeStatus: "ready", uptimeMs: 1 },
      }
    if (method === "conversation.list")
      return { ok: true, payload: { conversations: [], complete: true } }
    if (method === "auth.session") return { ok: true, payload: sessionReady }
    if (method === "credential.list")
      return { ok: true, payload: { credentials: devices } }
    if (method === "credential.revoke") {
      devices = devices.filter((item) => item.id !== params?.credentialId)
      return { ok: true, payload: { credentialId: params?.credentialId, revision: 1 } }
    }
    if (scenario.mode === "off")
      return { ok: false, error: { code: "pairing_not_configured", message: "off" } }
    if (scenario.mode === "refused")
      return { ok: false, error: { code: "forbidden", message: "no" } }
    if (method === "pairing.pending") return { ok: true, payload: { items: invitations } }
    if (method === "pairing.create") {
      if (invitations.some((item) => item.phase === "available"))
        return { ok: false, error: { code: "pairing_slot_occupied", message: "open" } }
      const created = status({
        expiresAtMs: scenario.expire ? 1 : Date.now() + 120_000,
      })
      invitations.push(created)
      return { ok: true, payload: { code: "ABCD-2345", status: created } }
    }
    const id = params?.invitationId
    const found = invitations.find((item) => same(item.invitationId, id))
    if (method === "pairing.cancel") {
      if (!found)
        return { ok: false, error: { code: "pairing_not_found", message: "gone" } }
      const at = invitations.indexOf(found)
      invitations.splice(at, 1)
      return {
        ok: true,
        payload: status({
          phase: "terminal",
          terminal: {
            cause: "cancelled",
            initiator: { kind: "principal", principalId: "owner" },
          },
        }),
      }
    }
    if (method === "pairing.approve") {
      if (!found)
        return { ok: false, error: { code: "pairing_not_found", message: "gone" } }
      found.phase = "approved"
      return {
        ok: true,
        payload: { status: found, activationStopped: "retryable" },
      }
    }
    return { ok: false, error: { code: "invalid_request", message: "not this check" } }
  }

  function route(socket) {
    const nonce = `linked-${Math.random().toString(16).slice(2)}`
    socket.send(
      JSON.stringify({
        type: "event",
        event: "session.challenge",
        seq: 1,
        stateVersion: 0,
        payload: { minVersion: 1, maxVersion: 1, nonce, expiresAt: 2_000_000_000 },
      }),
    )
    socket.onMessage((raw) => {
      let frame
      try {
        frame = JSON.parse(String(raw))
      } catch {
        return
      }
      if (!frame || typeof frame.id !== "string") return
      if (frame.method === "session.authenticate") {
        const matches = frame.params?.nonce === nonce
        socket.send(
          JSON.stringify(
            matches
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
      const result = reply(frame.method, frame.params)
      socket.send(JSON.stringify({ type: "res", id: frame.id, ...result }))
    })
  }

  return { route, unexpected }
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

async function showLinked(page) {
  if (!(await page.$(css.settings))) await page.keyboard.press(keys.settings)
  await need(page, css.settings, "Settings")
  await page.locator(css.settingsCategory, { hasText: names.connections }).click()
  await page.locator(css.settingsTab, { hasText: names.linkedDevices }).click()
  await page.waitForFunction(
    (selector) => {
      const phase = document.querySelector(selector)?.getAttribute("data-linked")
      return phase && phase !== "checking"
    },
    css.linked,
    { timeout: 15_000 },
  )
}

async function panelText(page) {
  return page.locator(css.settingsPanel).innerText()
}

function rawCodesIn(text) {
  return wireCodes.filter((code) => text.includes(code))
}

async function laidOut(page) {
  return page.evaluate(
    ({ panel, linked }) => {
      const bounds = document.querySelector(panel)?.getBoundingClientRect()
      const root = document.querySelector(linked)?.getBoundingClientRect()
      if (!bounds || !root) return ["the linked devices page was not measured"]
      const outside = []
      if (root.left < bounds.left - 1 || root.right > bounds.right + 1)
        outside.push("the page is wider than the settings panel")
      return outside
    },
    { panel: css.settingsPanel, linked: css.linked },
  )
}

const meta = {
  name: "linked-devices",
  summary: "Linked devices: off, refused, pair, approve, revoke, expired",
  defaults: { engine: "chromium,webkit" },
  help: `
Usage: node verification/desktop/scripts/linked-devices.mjs [options]

Per engine, against a scripted product session (and one sample page):
  pending    no gateway: the page is pending and offers no pairing
  off        linking off: disabled switch, how to turn it on, no pair control
  refused    this sign-in cannot pair; recover-owner is named
  pair       a code, both orbs, Cancel focused, then cancel clears it
  approve    a claimed device's fingerprint; Approve stays on a retryable stop
  revoke     a linked device is revoked only after confirm; the owner credential is not listed
  expired    a code already past its deadline reads as expired
  fit        the page is not wider than the settings panel
  codes      no wire code is on the page`,
}

await main(meta, async ({ options, rep, url }) => {
  await withEngines(options, rep, async (engine, browser) => {
    const step = (name, body) => attempt(rep, { engine, name }, body)

    await step("pending", async () => {
      const opened = await openPage(browser, {
        url,
        layout: "columns",
        width: 1200,
        height: 900,
      })
      try {
        await showLinked(opened.page)
        const phase = await opened.page.locator(css.linked).getAttribute("data-linked")
        const text = await panelText(opened.page)
        return {
          phase,
          failures: [
            phase === "pending" ? null : `phase was ${phase}, expected pending`,
            text.includes("Not available yet")
              ? null
              : "the pending sentence was missing",
            (await opened.page.locator(css.linkedAction("pair")).count()) === 0
              ? null
              : "a pair control was offered with no gateway",
            ...rawCodesIn(text).map((code) => `wire code on the page: ${code}`),
          ].filter(Boolean),
        }
      } finally {
        await opened.close()
      }
    })

    async function gatewayStep(name, scenario, check) {
      await step(name, async () => {
        const opened = await openGateway(browser, url, scenario)
        try {
          await showLinked(opened.page)
          const failures = await check(opened.page)
          const text = await panelText(opened.page)
          return {
            failures: [
              ...failures,
              ...rawCodesIn(text).map((code) => `wire code on the page: ${code}`),
              ...opened.gate.unexpected.map((method) => `unexpected method ${method}`),
            ],
          }
        } finally {
          await opened.close()
        }
      })
    }

    await gatewayStep("off", { mode: "off" }, async (page) => {
      const phase = await page.locator(css.linked).getAttribute("data-linked")
      const toggle = page.locator("[role='switch']")
      const text = await panelText(page)
      return [
        phase === "off" ? null : `phase was ${phase}, expected off`,
        (await toggle.getAttribute("aria-checked")) === "false"
          ? null
          : "the switch read on",
        (await toggle.isDisabled()) ? null : "the switch could be turned",
        text.includes("127.0.0.1:47650") ? null : "the enable hint was missing",
        (await page.locator(css.linkedAction("pair")).count()) === 0
          ? null
          : "pair was offered while linking is off",
      ].filter(Boolean)
    })

    await gatewayStep("refused", { mode: "refused" }, async (page) => {
      const phase = await page.locator(css.linked).getAttribute("data-linked")
      const text = await panelText(page)
      return [
        phase === "refused" ? null : `phase was ${phase}, expected refused`,
        text.includes("auth recover-owner") ? null : "recover-owner was not named",
        (await page.locator(css.linkedAction("pair")).count()) === 0
          ? null
          : "pair was offered to a refused sign-in",
        (await page.locator("[data-credential]").count()) === 0
          ? null
          : "a device was listed for a refused sign-in",
      ].filter(Boolean)
    })

    await gatewayStep("pair", { mode: "on" }, async (page) => {
      await page.locator(css.linkedAction("pair")).click()
      await page.waitForFunction(
        (selector) => {
          const button = document.querySelector(selector)
          return button && !button.disabled && document.activeElement === button
        },
        css.linkedAction("cancel"),
        { timeout: 10_000 },
      )
      const shown = (
        await page.locator('[data-slot="pairing-code-value"]').innerText()
      ).replace(/\s+/g, "")
      const orbs = {
        signal: await page.locator(css.signalOrb).count(),
        qr: await page.locator(css.qrOrb).count(),
      }
      const fit = await laidOut(page)
      await page.locator(css.linkedAction("cancel")).click()
      await page.waitForSelector(css.pairingCode, { state: "detached", timeout: 10_000 })
      return [
        shown === "ABCD2345" ? null : `the code was ${shown}`,
        orbs.signal === 1 ? null : `signal orbs: ${orbs.signal}`,
        orbs.qr === 1 ? null : `qr orbs: ${orbs.qr}`,
        ...fit,
      ].filter(Boolean)
    })

    await gatewayStep(
      "approve",
      {
        mode: "on",
        invitations: [status({ phase: "claimed", claimedDeviceKey: deviceKey })],
      },
      async (page) => {
        await page.waitForSelector(css.fingerprint, { timeout: 10_000 })
        const before = await panelText(page)
        await page.locator(css.linkedAction("approve")).click()
        await page.waitForSelector(css.linkedNotice, { timeout: 10_000 })
        const after = await panelText(page)
        return [
          before.includes("182F") ? null : "the fingerprint was not shown",
          after.includes("Try again") ? null : "the retryable stop was not said",
          (await page.locator(css.linkedAction("approve")).count()) === 1
            ? null
            : "Approve was withdrawn",
        ].filter(Boolean)
      },
    )

    await gatewayStep(
      "revoke",
      {
        mode: "on",
        devices: [
          credential("owner-credential", ["conversation.read", "credential.manage"]),
          credential("device-1", ["conversation.read"]),
        ],
      },
      async (page) => {
        const rows = await page
          .locator("[data-credential]")
          .evaluateAll((nodes) =>
            nodes.map((node) => node.getAttribute("data-credential")),
          )
        await page.locator(css.linkedAction("revoke")).click()
        const asked = await panelText(page)
        await page.locator(css.linkedAction("confirm-revoke")).click()
        await page.waitForSelector(css.linkedNotice, { timeout: 10_000 })
        const after = await panelText(page)
        return [
          rows.includes("owner-credential") ? "the session credential was listed" : null,
          rows.includes("device-1") ? null : "the linked device was not listed",
          asked.includes("Revoke this device?") ? null : "revoke was not asked",
          after.includes("next time it reads") ? null : "the revoke sentence was missing",
        ].filter(Boolean)
      },
    )

    await gatewayStep("expired", { mode: "on", expire: true }, async (page) => {
      await page.locator(css.linkedAction("pair")).click()
      await page.waitForSelector(`${css.pairingCode}[data-state="expired"]`, {
        timeout: 10_000,
      })
      const text = await panelText(page)
      return [
        text.includes("This code has expired")
          ? null
          : "the expired sentence was missing",
      ].filter(Boolean)
    })
  })
})
