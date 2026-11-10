#!/usr/bin/env node
/** #893: a real Integration panel, explicit Human sign-in, immutable PeerRead
 * choice, crash recovery and native enrollment. Only the creation/approval UI
 * under test mutates invitations; fixture setup also ends one invitation to
 * exercise terminal polling. CLI calls enroll a disposable peer. */
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
function requireValue(condition, message) {
  if (!condition) throw new Error(message)
}
async function enter(page) {
  if (!(await page.locator(css.devicesAddAction).count()))
    await page.locator(selectorFor.railItem("devices")).click()
  await page.locator(css.devicesAddAction).click()
  await page.locator(css.devicesGatewayTile).click()
  await page.locator(css.gatewayPairingPage).waitFor()
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
            const sessions = []
            try {
              owner = await connectClient(
                stack.gateway,
                `pairing-owner-${engine}-${layout}`,
              )
              const human = (await owner.auth.session()).principalId
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
                      const seen = { client: undefined, closed: false }
                      socket.on("framesent", ({ payload }) => {
                        try {
                          const frame = JSON.parse(payload)
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
                !(await page.locator('[type="checkbox"]').count()),
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
              const row = page.locator(`[data-credential="${pending.credentialId}"]`)
              await row.locator('[data-linked-action="revoke"]').click()
              await row.locator('[data-linked-action="confirm-revoke"]').click()
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
                `[data-credential="${replacement.credentialId}"]`,
              )
              await replacementRow.locator('[data-linked-action="revoke"]').click()
              await replacementRow
                .locator('[data-linked-action="confirm-revoke"]')
                .click()
              await replacementRow.waitFor({ state: "hidden" })
              return {
                true: 2,
                false: 2,
                integrationRefused: 1,
                refresh: 1,
                nativeActive: 1,
                replacementFalseActive: 1,
                stalePersonLinks: 0,
                polledTerminalSessionClosed: true,
                currentDocumentTemporarySessionsClosed: true,
              }
            } finally {
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
