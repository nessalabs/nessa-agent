#!/usr/bin/env node
/** #893: a real Integration panel, explicit Human sign-in, immutable PeerRead
 * choice, crash recovery and native enrollment. Only the creation/approval UI
 * under test mutates invitations. Fixture-only writes exercise terminal polling,
 * audit failures and a permanent receiver conflict. CLI enrolls a disposable peer. */
import { TEXT_REPLY_SCENARIO } from "../../../scripts/mcp-test-server/scenarios.mjs"
import { DatabaseSync } from "node:sqlite"
import { spawn } from "node:child_process"
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { startLocalGateway } from "../../../scripts/mcp-test-server/local-gateway.mjs"
import { attempt } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import {
  connectClient,
  panelCredential,
  startGatewayStack,
  waitFor,
} from "./lib/gateway-stack.mjs"
import { freePort } from "./lib/server.mjs"
import { css, selectorFor, storage } from "./lib/selectors.mjs"
import { main } from "./lib/run.mjs"

const meta = {
  name: "gateway-pairing",
  summary: "PeerRead invitation choice and explicit Human pairing scope",
  defaults: { engine: "chromium,webkit", layout: "columns,sidebar" },
  options: { agent: { type: "string", default: "claude" }, evidence: { type: "string" } },
  help: "Gateway pairing with disposable native gateways. --evidence DIR saves the changed surface. --mode prod supported.",
}
const receiverInsertConflict =
  "CREATE TRIGGER pairing_ui_receiver_conflict BEFORE INSERT ON receivers BEGIN INSERT INTO receivers (receiver_id,credential_id,organization_id,owner_id,access_epoch,active) VALUES (NEW.receiver_id,NEW.credential_id,NEW.organization_id,NEW.owner_id,NEW.access_epoch,NEW.active); END"

function requireValue(condition, message) {
  if (!condition) throw new Error(message)
}
async function focusHeld(locator, message) {
  requireValue(
    await waitFor(
      () => locator.evaluate((element) => element === document.activeElement),
      5_000,
    ),
    message,
  )
}
async function enter(page) {
  if (!(await page.locator(css.devicesAddAction).count()))
    await page.locator(selectorFor.railItem("devices")).click()
  await page.locator(css.devicesAddAction).click()
  await page.locator(css.devicesGatewayTile).click()
  await page.locator(css.gatewayPairingPage).waitFor()
  await focusHeld(
    page.locator(css.gatewayPairingPage),
    "gateway entry did not receive focus",
  )
}
async function signIn(page, token, resume = false) {
  if (token !== undefined) {
    if (resume)
      await page
        .getByRole("button", { name: "Resume owner invitations", exact: true })
        .click()
    else await page.getByRole("checkbox", { name: "Both gateways belong to me" }).check()
    await page.getByLabel("Owner access token").fill(token)
  }
  await page.getByRole("button", { name: "Continue", exact: true }).click()
  await page.getByRole("button", { name: "Read pending invitations" }).waitFor()
  if (!resume)
    await focusHeld(
      page.getByRole("button", { name: "Create invitation", exact: true }),
      "ready invitation did not focus Create",
    )
}
function call(gateway, instance, method, params) {
  return new Promise((resolve, reject) => {
    const proc = spawn(
      process.env.MCP_LIVE_NESSA,
      [
        "call",
        method,
        "--params",
        JSON.stringify(params),
        "--credential-file",
        gateway.token,
      ],
      {
        env: {
          ...process.env,
          NESSA_STAGE: "ci",
          NESSA_INSTANCE: instance,
          NESSA_DATA_DIR: gateway.directory,
          NESSA_PORT: new URL(gateway.url).port,
        },
        stdio: ["ignore", "pipe", "pipe"],
      },
    )
    let output = ""
    proc.stdout.on("data", (chunk) => {
      output += chunk
    })
    // Credentials/codes and private error dumps do not become test receipts.
    proc.stderr.resume()
    proc.on("error", reject)
    proc.on("exit", (code) => {
      if (code !== 0) return reject(new Error(`fixture ${method} failed (${code})`))
      try {
        resolve(JSON.parse(output))
      } catch {
        reject(new Error(`fixture ${method} did not return JSON`))
      }
    })
  })
}

await main(
  meta,
  async ({ options, rep, target: stack }) => {
    await withEngines(options, rep, async (engine, browser) => {
      for (const layout of options.layouts) {
        await attempt(
          rep,
          { name: "choice-human-recovery-native", engine, layout },
          async () => {
            const credential = panelCredential(stack.gateway)
            const peerInstance = `pairing-peer-${engine}-${layout}`
            const peer = await startLocalGateway({
              agent: "claude",
              model: "claude-sonnet-5",
              agentArgv: [process.execPath, "-e", "process.exit(1)"],
              mcpServers: [],
              signedOut: true,
              instance: peerInstance,
              port: await freePort(),
              native: { enabled: true, listenAddress: `127.0.0.1:${await freePort()}` },
            })
            let opened
            let owner
            let grants
            let receivers
            const sessions = []
            try {
              owner = await connectClient(
                stack.gateway,
                `pairing-owner-${engine}-${layout}`,
              )
              const ownerSession = await owner.auth.session()
              const human = ownerSession.principalId
              requireValue(
                (await stack.client.auth.session()).principalId === "surface:nessa-panel",
                "fixture panel is not the real Integration",
              )
              opened = await openPage(browser, {
                url: `${new URL(stack.url).origin}/desktop.html`,
                layout,
                prefs: { [storage.sideRail]: "on" },
                initScripts: [
                  [
                    gatewayHost,
                    { endpoint: stack.gateway.url.replace(/^http/, "ws"), credential },
                  ],
                ],
                beforeLoad: (context) =>
                  context.on("page", (page) =>
                    page.on("websocket", (socket) => {
                      const seen = { client: undefined, closed: false, statusReads: 0 }
                      socket.on("framesent", ({ payload }) => {
                        try {
                          const frame = JSON.parse(payload)
                          if (frame.method === "pairing.status") seen.statusReads += 1
                          if (frame.method === "session.authenticate") {
                            seen.client = frame.params.client.id
                            sessions.push(seen)
                          }
                        } catch {
                          /* Ignore non-JSON transport frames, not authentication results. */
                        }
                      })
                      socket.on("close", () => {
                        seen.closed = true
                      })
                    }),
                  ),
              })
              const { page } = opened
              await enter(page)
              requireValue(
                (await stack.client.pairing.pending()).items.length === 0,
                "opening page created an invitation",
              )
              await signIn(page)
              await page.getByRole("button", { name: "Create invitation" }).click()
              await page.locator(css.gatewayPairingCode).waitFor()
              await focusHeld(
                page.locator(css.gatewayPairingPage),
                "created invitation did not receive focus",
              )
              const commandBox = await page
                .locator(css.gatewayPairingCommand)
                .evaluate((element) => ({
                  width: element.clientWidth,
                  scrollWidth: element.scrollWidth,
                }))
              requireValue(
                commandBox.width > 0 && commandBox.scrollWidth <= commandBox.width + 1,
                "native enrollment command overflowed its surface",
              )
              const constrainedCommandBox = await page
                .locator(css.gatewayPairingCommand)
                .evaluate((element) => {
                  const width = element.style.width
                  element.style.width = "240px"
                  const measured = {
                    width: element.clientWidth,
                    scrollWidth: element.scrollWidth,
                  }
                  element.style.width = width
                  return measured
                })
              requireValue(
                constrainedCommandBox.width === 240 &&
                  constrainedCommandBox.scrollWidth <= constrainedCommandBox.width + 1,
                "native enrollment command overflowed its constrained surface",
              )
              const focusedCancel = page.getByRole("button", {
                name: "Cancel invitation",
                exact: true,
              })
              await focusedCancel.focus()
              const statusReads = () =>
                sessions.reduce((count, each) => count + each.statusReads, 0)
              const beforeRead = statusReads()
              requireValue(
                await waitFor(() => statusReads() > beforeRead, 5_000),
                "selected invitation did not poll",
              )
              await focusHeld(
                focusedCancel,
                "a status poll stole focus from the selected control",
              )
              requireValue(
                (await page.locator(css.gatewayPairingChoice).innerText()).includes(
                  "no person link",
                ),
                "false choice was lost",
              )
              await page
                .getByRole("button", { name: "Cancel invitation", exact: true })
                .click()
              await page
                .getByRole("button", { name: "End pairing sign-in" })
                .waitFor({ state: "hidden" })
              await page.getByRole("button", { name: "Back", exact: true }).click()
              await enter(page)
              // An actual Integration credential cannot masquerade as Human.
              await signIn(page, credential)
              await page.getByRole("button", { name: "Create invitation" }).click()
              await page.getByRole("alert").waitFor()
              requireValue(
                (await stack.client.pairing.pending()).items.length === 0,
                "ineligible proof registered an invitation",
              )
              requireValue(
                (await owner.pairing.pending()).items.length === 0,
                "ineligible proof changed the Human owner's invitations",
              )
              await page.getByRole("button", { name: "End pairing sign-in" }).click()
              if (options.evidence) {
                mkdirSync(options.evidence, { recursive: true })
                await page.locator(css.gatewayPairingPage).screenshot({
                  path: join(options.evidence, `gateway-choice-${engine}-${layout}.png`),
                })
              }
              await signIn(page, stack.token())
              await page.getByRole("button", { name: "Create invitation" }).click()
              await page.locator(css.gatewayPairingCode).waitFor()
              const endedHex = await page
                .locator(css.gatewayPairingInvitation)
                .getAttribute("data-gateway-invitation")
              const endedSession = sessions
                .filter((each) => each.client === "nessa-pairing")
                .at(-1)
              await owner.pairing.cancel(Array.from(Buffer.from(endedHex, "hex")))
              await page
                .locator(css.gatewayPairingPhase)
                .filter({ hasText: "Ended" })
                .waitFor()
              await page
                .getByRole("button", { name: "End pairing sign-in" })
                .waitFor({ state: "hidden" })
              requireValue(
                !(await page.locator(css.gatewayPairingCode).count()),
                "terminal polling retained the one-use code",
              )
              requireValue(
                await waitFor(async () => endedSession?.closed, 5_000),
                "terminal polling retained the Human session",
              )
              await page.getByRole("button", { name: "Back", exact: true }).click()
              await enter(page)
              await signIn(page, stack.token())
              await page.getByRole("button", { name: "Create invitation" }).click()
              const code = await page.locator(css.gatewayPairingCode).innerText()
              const invitationHex = await page
                .locator(css.gatewayPairingInvitation)
                .getAttribute("data-gateway-invitation")
              const invitation = Array.from(Buffer.from(invitationHex, "hex"))
              requireValue(
                (await owner.pairing.status(invitation)).samePerson === true,
                "canonical true choice missing",
              )
              requireValue(
                !(await page.getByRole("checkbox", { includeHidden: true }).count()),
                "created choice is still editable",
              )
              if (options.evidence) {
                mkdirSync(options.evidence, { recursive: true })
                await page.locator(css.gatewayPairingPage).screenshot({
                  path: join(options.evidence, `gateway-${engine}-${layout}.png`),
                })
              }
              const secretInStorage = await page.evaluate(
                (secret) =>
                  [
                    ...Object.values(localStorage),
                    ...Object.values(sessionStorage),
                    location.href,
                  ].some((value) => value.includes(secret)),
                stack.token(),
              )
              requireValue(
                !secretInStorage,
                "owner credential persisted in browser storage or URL",
              )
              // Refresh retires the privileged socket. A new explicit sign-in reads
              // the immutable invitation; the code is not reconstructed.
              const previousDocumentSessions = sessions.length
              await page.reload()
              await enter(page)
              await signIn(page, stack.token(), true)
              const resume = page.getByRole("button", {
                name: new RegExp(`Resume ${invitationHex}`),
              })
              await resume.click()
              await focusHeld(
                page.locator(css.gatewayPairingPage),
                "resumed invitation did not receive focus",
              )
              requireValue(
                (await page.locator(css.gatewayPairingChoice).innerText()).includes(
                  "same person",
                ),
                "recovered choice changed",
              )
              requireValue(
                !(await page.locator(css.gatewayPairingCode).count()),
                "refresh reconstructed a secret code",
              )
              const enrolled = await call(peer, peerInstance, "peer.enroll", {
                address: stack.nativeAddress,
                code,
              })
              await page.locator(css.gatewayPairingFingerprint).waitFor()
              grants = new DatabaseSync(
                join(
                  stack.gateway.directory,
                  "ci",
                  "instances",
                  "gateway-pairing",
                  "access-grants",
                  "access-grants.sqlite3",
                ),
              )
              grants.exec(
                "CREATE TRIGGER pairing_ui_fail_audit BEFORE INSERT ON person_link_changes BEGIN SELECT RAISE(FAIL, 'fixture audit unavailable'); END",
              )
              await page
                .getByRole("button", { name: "Approve matching fingerprint" })
                .click()
              await page
                .getByRole("status")
                .filter({ hasText: "Activation needs another attempt" })
                .waitFor()
              const stopped = await owner.pairing.status(invitation)
              requireValue(
                stopped.phase === "active",
                "credential did not commit before the failed audit write",
              )
              requireValue(
                grants
                  .prepare("SELECT count(*) AS n FROM person_links WHERE credential_id=?")
                  .get(stopped.credentialId).n === 0,
                "person linked before its audit committed",
              )
              requireValue(
                grants
                  .prepare(
                    "SELECT count(*) AS n FROM person_link_changes WHERE credential_id=?",
                  )
                  .get(stopped.credentialId).n === 0,
                "failed audit write left a receipt",
              )
              grants.exec("DROP TRIGGER pairing_ui_fail_audit")
              await page.getByRole("button", { name: "Retry activation" }).click()
              await page
                .getByRole("button", { name: "End pairing sign-in" })
                .waitFor({ state: "hidden" })
              requireValue(
                (await owner.pairing.status(invitation)).phase === "active",
                "native peer did not activate",
              )
              requireValue(
                await waitFor(
                  async () =>
                    sessions
                      .slice(previousDocumentSessions)
                      .filter((each) => each.client === "nessa-pairing")
                      .every((each) => each.closed),
                  5_000,
                ),
                `privileged pairing socket remained open: ${JSON.stringify(sessions)}`,
              )
              requireValue(
                sessions.some((each) => each.client === "nessa-panel" && !each.closed),
                "ordinary panel session was replaced or closed",
              )
              // This assertion is about actual authenticated ownership, not a label.
              const pending = await owner.pairing.status(invitation)
              requireValue(
                pending.credentialId,
                "active enrollment omitted its credential",
              )
              const listed = (await owner.credentials.list()).credentials.find(
                (each) => each.id === pending.credentialId,
              )
              requireValue(
                pending.class === "peer-gateway-conversation-read" &&
                  listed?.principalId.startsWith("gateway:"),
                "peer credential class/principal changed",
              )
              requireValue(
                listed?.principalId !== human,
                "same person replaced the grant subject",
              )
              requireValue(
                pending.credentialId === stopped.credentialId &&
                  pending.receiver.receiverId === stopped.receiver.receiverId,
                "retry replaced credential or receiver",
              )
              const link = grants
                .prepare(
                  "SELECT person_principal FROM person_links WHERE credential_id=?",
                )
                .get(pending.credentialId)
              requireValue(
                link?.person_principal === human,
                "committed link does not name the authenticated Human",
              )
              const attribution = grants
                .prepare(
                  "SELECT person_before,person_after,initiator_kind,initiator_id FROM person_link_changes WHERE credential_id=? ORDER BY sequence",
                )
                .all(pending.credentialId)
              requireValue(
                attribution.length === 1 &&
                  attribution[0].person_before === null &&
                  attribution[0].person_after === human &&
                  attribution[0].initiator_kind === "principal" &&
                  attribution[0].initiator_id === human,
                "attribution audit lost the actual Human actor or before/after",
              )
              requireValue(
                grants.prepare("SELECT count(*) AS n FROM share_grants").get().n === 0,
                "same person created a share grant",
              )
              await page.getByRole("button", { name: "Back", exact: true }).click()
              await page.locator(css.devicesAddAction).click()
              await page.locator(css.devicesPhoneTile).click()
              const row = page.locator(selectorFor.linkedCredential(pending.credentialId))
              await row.locator(selectorFor.linkedAction("revoke")).click()
              await row.locator(selectorFor.linkedAction("confirm-revoke")).click()
              await row.waitFor({ state: "hidden" })
              requireValue(
                grants
                  .prepare("SELECT count(*) AS n FROM person_links WHERE credential_id=?")
                  .get(pending.credentialId).n === 0,
                "UI credential revoke retained attribution",
              )
              const removal = grants
                .prepare(
                  "SELECT person_before,person_after,initiator_kind,initiator_id FROM person_link_changes WHERE credential_id=? ORDER BY sequence DESC LIMIT 1",
                )
                .get(pending.credentialId)
              requireValue(
                removal?.person_before === human &&
                  removal.person_after === null &&
                  removal.initiator_kind === "principal" &&
                  removal.initiator_id === "surface:nessa-panel",
                "UI revoke audit impersonated the Human or lost before/after",
              )
              // The same native key, enrolled again without the choice, must
              // not inherit the retired credential's person association.
              await call(peer, peerInstance, "peer.forget", { peerKey: enrolled.peerKey })
              await page.locator(css.devicesBackAction).click()
              await enter(page)
              await signIn(page)
              await page.getByRole("button", { name: "Create invitation" }).click()
              const replacementCode = await page
                .locator(css.gatewayPairingCode)
                .innerText()
              const replacementHex = await page
                .locator(css.gatewayPairingInvitation)
                .getAttribute("data-gateway-invitation")
              const replacementId = Array.from(Buffer.from(replacementHex, "hex"))
              await call(peer, peerInstance, "peer.enroll", {
                address: stack.nativeAddress,
                code: replacementCode,
              })
              await page.locator(css.gatewayPairingFingerprint).waitFor()
              await page
                .getByRole("button", { name: "Approve matching fingerprint" })
                .click()
              await page
                .getByRole("button", { name: "End pairing sign-in" })
                .waitFor({ state: "hidden" })
              const replacement = await stack.client.pairing.status(replacementId)
              requireValue(
                replacement.phase === "active" && replacement.samePerson === false,
                "replacement did not retain false through activation",
              )
              requireValue(
                replacement.credentialId !== pending.credentialId,
                "replacement reused the retired credential",
              )
              const replacementCredential = (
                await stack.client.credentials.list()
              ).credentials.find((each) => each.id === replacement.credentialId)
              requireValue(
                replacementCredential?.principalId === listed.principalId,
                "replacement changed the gateway principal",
              )
              requireValue(
                grants
                  .prepare(
                    "SELECT count(*) AS n FROM person_links WHERE gateway_principal=?",
                  )
                  .get(listed.principalId).n === 0,
                "later false credential inherited a person link",
              )
              await page.getByRole("button", { name: "Back", exact: true }).click()
              await page.locator(css.devicesAddAction).click()
              await page.locator(css.devicesPhoneTile).click()
              const replacementRow = page.locator(
                selectorFor.linkedCredential(replacement.credentialId),
              )
              await replacementRow.locator(selectorFor.linkedAction("revoke")).click()
              await replacementRow
                .locator(selectorFor.linkedAction("confirm-revoke"))
                .click()
              await replacementRow.waitFor({ state: "hidden" })
              // A fixture-only receiver insert collision exercises the real
              // permanent classification. Its transaction rolls back both rows.
              await call(peer, peerInstance, "peer.forget", { peerKey: enrolled.peerKey })
              await page.locator(css.devicesBackAction).click()
              await enter(page)
              await signIn(page, stack.token())
              await page.getByRole("button", { name: "Create invitation" }).click()
              const conflictCode = await page.locator(css.gatewayPairingCode).innerText()
              const conflictHex = await page
                .locator(css.gatewayPairingInvitation)
                .getAttribute("data-gateway-invitation")
              const conflictId = Array.from(Buffer.from(conflictHex, "hex"))
              await call(peer, peerInstance, "peer.enroll", {
                address: stack.nativeAddress,
                code: conflictCode,
              })
              await page.locator(css.gatewayPairingFingerprint).waitFor()
              receivers = new DatabaseSync(
                join(
                  stack.gateway.directory,
                  "ci",
                  "instances",
                  "gateway-pairing",
                  "receiver-access",
                  "receiver-access.sqlite3",
                ),
              )
              receivers.exec(receiverInsertConflict)
              await page
                .getByRole("button", { name: "Approve matching fingerprint" })
                .click()
              await page
                .getByRole("status")
                .filter({ hasText: "Activation cannot complete" })
                .waitFor()
              requireValue(
                !(await page.getByRole("button", { name: "Retry activation" }).count()),
                "permanent activation still offered retry",
              )
              requireValue(
                await page
                  .getByRole("button", { name: "Cancel invitation", exact: true })
                  .isEnabled(),
                "permanent activation omitted cancellation",
              )
              await focusHeld(
                page.locator(css.gatewayPairingPage),
                "permanent activation did not receive focus",
              )
              const conflict = await owner.pairing.status(conflictId)
              requireValue(
                conflict.phase === "staging" &&
                  conflict.credentialId &&
                  !conflict.receiver,
                "receiver conflict did not stop before publication",
              )
              requireValue(
                !(await owner.credentials.list()).credentials.some(
                  (each) => each.id === conflict.credentialId,
                ),
                "permanent receiver conflict published its reserved credential",
              )
              requireValue(
                receivers
                  .prepare("SELECT count(*) AS n FROM receivers WHERE credential_id=?")
                  .get(conflict.credentialId).n === 0,
                "receiver conflict did not roll back its transaction",
              )
              // End without cancelling through this page. Fixture-owner cleanup
              // permits a new invitation; its UI must not inherit the old stop.
              await page.getByRole("button", { name: "End pairing sign-in" }).click()
              await focusHeld(
                page.locator(css.gatewayPairingPage),
                "ending pairing did not focus its choices",
              )
              receivers.exec("DROP TRIGGER pairing_ui_receiver_conflict")
              const abandoned = await owner.pairing.cancel(conflictId)
              requireValue(
                abandoned.phase === "terminal" && !abandoned.cleanupPending,
                "fixture owner did not retire the abandoned stage",
              )
              await call(peer, peerInstance, "peer.forget", { peerKey: enrolled.peerKey })
              await signIn(page, stack.token())
              await page.getByRole("button", { name: "Create invitation" }).click()
              await page.locator(css.gatewayPairingCode).waitFor()
              requireValue(
                !(await page
                  .getByRole("status")
                  .filter({ hasText: "Activation cannot complete" })
                  .count()),
                "new invitation inherited a previous permanent activation stop",
              )
              const freshCode = await page.locator(css.gatewayPairingCode).innerText()
              const freshHex = await page
                .locator(css.gatewayPairingInvitation)
                .getAttribute("data-gateway-invitation")
              const freshId = Array.from(Buffer.from(freshHex, "hex"))
              await call(peer, peerInstance, "peer.enroll", {
                address: stack.nativeAddress,
                code: freshCode,
              })
              await page.locator(css.gatewayPairingFingerprint).waitFor()
              requireValue(
                await page
                  .getByRole("button", { name: "Approve matching fingerprint" })
                  .isEnabled(),
                "new claimed invitation lost approval after an earlier permanent stop",
              )
              receivers.exec(receiverInsertConflict)
              await page
                .getByRole("button", { name: "Approve matching fingerprint" })
                .click()
              await page
                .getByRole("status")
                .filter({ hasText: "Activation cannot complete" })
                .waitFor()
              receivers.exec("DROP TRIGGER pairing_ui_receiver_conflict")
              await page
                .getByRole("button", { name: "Cancel invitation", exact: true })
                .click()
              await page
                .getByRole("button", { name: "End pairing sign-in" })
                .waitFor({ state: "hidden" })
              const cancelled = await owner.pairing.status(freshId)
              requireValue(
                cancelled.phase === "terminal" &&
                  !cancelled.cleanupPending &&
                  cancelled.terminal?.cause === "cancelled" &&
                  cancelled.terminal.initiator.kind === "principal" &&
                  cancelled.terminal.initiator.principalId === human,
                "permanent activation lost its Human cancellation receipt",
              )
              for (const ended of [abandoned, cancelled]) {
                requireValue(
                  ended.credentialId &&
                    grants
                      .prepare(
                        "SELECT count(*) AS n FROM person_links WHERE credential_id=?",
                      )
                      .get(ended.credentialId).n === 0 &&
                    grants
                      .prepare(
                        "SELECT count(*) AS n FROM person_link_changes WHERE credential_id=?",
                      )
                      .get(ended.credentialId).n === 0,
                  "unpublished permanent stop inserted attribution or a false link receipt",
                )
              }
              // Fail one private status send, with no server mutation or fake
              // authority. Recovery must name a control the person can use.
              await page.getByRole("button", { name: "Back", exact: true }).click()
              await enter(page)
              await signIn(page, stack.token())
              await page.getByRole("button", { name: "Create invitation" }).click()
              await page.locator(css.gatewayPairingCode).waitFor()
              const readHex = await page
                .locator(css.gatewayPairingInvitation)
                .getAttribute("data-gateway-invitation")
              await page.evaluate(() => {
                const send = WebSocket.prototype.send
                WebSocket.prototype.send = function (data) {
                  let method
                  try {
                    method = JSON.parse(String(data)).method
                  } catch {
                    // Non-JSON transport data remains untouched.
                  }
                  if (method === "pairing.status") {
                    WebSocket.prototype.send = send
                    throw new Error("Fixture status transport unavailable")
                  }
                  return send.call(this, data)
                }
              })
              await page
                .getByRole("alert")
                .filter({ hasText: "Go back and reopen Gateway" })
                .waitFor()
              requireValue(
                await page.getByRole("button", { name: "Back", exact: true }).isEnabled(),
                "failed selected read prescribed unavailable recovery",
              )
              await page.getByRole("button", { name: "Back", exact: true }).click()
              await enter(page)
              await signIn(page, stack.token(), true)
              await page
                .getByRole("button", { name: new RegExp(`Resume ${readHex}`) })
                .click()
              requireValue(
                !(await page.locator(css.gatewayPairingCode).count()),
                "read recovery reconstructed a one-use code",
              )
              await page
                .getByRole("button", { name: "Cancel invitation", exact: true })
                .click()
              await page
                .getByRole("button", { name: "End pairing sign-in" })
                .waitFor({ state: "hidden" })
              requireValue(
                (await owner.pairing.status(Array.from(Buffer.from(readHex, "hex"))))
                  .phase === "terminal",
                "read recovery did not cancel the same invitation",
              )
              await opened.settleRequests()
              requireValue(
                opened.heldErrorCount === 0,
                "pairing page reported unexpected browser errors",
              )
              return {
                focusHeld: true,
                pollPreservedFocus: true,
                commandBox,
                constrainedCommandBox,
                pageErrors: opened.heldErrorCount,
                true: 5,
                false: 2,
                integrationRefused: 1,
                refresh: 1,
                nativeActive: 1,
                replacementFalseActive: 1,
                stalePersonLinks: 0,
                failedReadRecovered: 1,
                nativePermanentStop: 2,
                stalePermanentStopAbsent: true,
                permanentStopCancelledWithoutLink: true,
                polledTerminalSessionClosed: true,
                currentDocumentTemporarySessionsClosed: true,
              }
            } finally {
              if (receivers) {
                receivers.exec("DROP TRIGGER IF EXISTS pairing_ui_receiver_conflict")
                receivers.close()
              }
              if (grants) {
                grants.exec("DROP TRIGGER IF EXISTS pairing_ui_fail_audit")
                grants.close()
              }
              owner?.close()
              await opened?.close()
              await peer.stop()
            }
          },
        )
      }
    })
  },
  async (options) => {
    const nativeAddress = `127.0.0.1:${await freePort()}`
    const stack = await startGatewayStack(options, "gateway-pairing", {
      as: "panel",
      scenario: TEXT_REPLY_SCENARIO,
      native: { enabled: true, listenAddress: nativeAddress },
    })
    return { ...stack, nativeAddress }
  },
)
