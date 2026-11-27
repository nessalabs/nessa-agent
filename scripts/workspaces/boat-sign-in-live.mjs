#!/usr/bin/env node
/**
 * Sign in with Boat against the real Boat, through a temporary gateway built
 * from this checkout. Opt-in and by hand: it opens Boat's page in this
 * machine's browser and waits (up to 10 minutes) for the person to approve.
 *
 * ```sh
 * cargo build -p nessa-server --bin nessa
 * node scripts/workspaces/boat-sign-in-live.mjs
 * ```
 *
 * It checks the released app (`protocol/defaults/boat-app.json`): Boat takes
 * its client id, scopes and a loopback redirect on a port of the gateway's
 * choosing; the gateway exchanges the code, reads the account, validates the
 * `nessa` environment and saves the sign-in; status names the account; and
 * Disconnect revokes it. It prints only states and the account's login, never
 * a token. Creates no VM.
 */
import assert from "node:assert/strict"
import { execFile } from "node:child_process"
import { randomUUID } from "node:crypto"
import { platform } from "node:os"
import { setTimeout as sleep } from "node:timers/promises"
import { startLocalGateway } from "../mcp-test-server/local-gateway.mjs"
import {
  connectClient,
  scriptedLaunch,
} from "../../verification/desktop/scripts/lib/gateway-stack.mjs"
import { freePort } from "../../verification/desktop/scripts/lib/server.mjs"

const passphrase = `Disposable boat sign-in ${randomUUID()}`
const instance = `boat-sign-in-${randomUUID()}`
const command = scriptedLaunch("claude", { tool: "read_only" }).command
const open = (url) =>
  execFile(platform() === "darwin" ? "open" : "xdg-open", [url], () => {})
const say = (line) => console.log(`boat-sign-in-live: ${line}`)

let gateway, client
try {
  gateway = await startLocalGateway({
    agent: "claude",
    port: await freePort(),
    instance,
    agentArgv: command.argv,
    stagedRuntime: command.stagedRuntime,
    semanticArgs: command.semanticArgs,
    model: command.model,
    signedOut: true,
    mcpServers: [],
  })
  client = await connectClient(gateway, instance, "panel")
  await client.vault.setup(passphrase)
  const before = await client.boat.status()
  assert.equal(before.state, "disconnected")
  assert.equal(before.signInAvailable, true, "this build names no Boat app")
  const started = await client.boat.signIn()
  assert.equal(started.signIn?.state, "waiting")
  const url = new URL(started.signIn.url)
  assert.equal(url.origin, "https://boat.dev")
  say(
    `opening Boat (client ${url.searchParams.get("client_id")}, redirect ${url.searchParams.get("redirect_uri")})`,
  )
  say("approve Nessa in the browser; waiting up to 10 minutes")
  open(url.toString())
  let status = started
  for (
    let waited = 0;
    status.signIn?.state === "waiting" && waited < 610_000;
    waited += 2000
  ) {
    await sleep(2000)
    status = await client.boat.status()
  }
  if (status.state !== "connected")
    throw new Error(`sign-in ended ${JSON.stringify(status.signIn ?? status.state)}`)
  assert.equal(status.via, "signIn")
  say(`connected as ${status.login} (environment ${status.environment})`)
  // A second read goes through the renewing settings; still connected.
  assert.equal((await client.boat.status()).state, "connected")
  const after = await client.boat.disconnect()
  assert.equal(after.state, "disconnected")
  say("disconnected; the refresh token was revoked in Boat")
  say("PASS")
} catch (error) {
  process.exitCode = 1
  say(`FAIL ${error.message}`)
} finally {
  await client?.close?.().catch(() => {})
  await gateway?.stop?.().catch(() => {})
}
