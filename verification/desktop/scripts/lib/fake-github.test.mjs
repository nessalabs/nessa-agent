/**
 * The fake GitHub answers the device flow as GitHub documents it: pending
 * until approved, the token once, and the account only for that token.
 */
import { strict as assert } from "node:assert"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import { FAKE_ACCOUNT, startFakeGithub } from "./fake-github.mjs"

test("a code is pending until approved, then gives its token once, and the token reads the account", async (t) => {
  const github = await startFakeGithub()
  t.after(() => github.close())
  const dir = mkdtempSync(join(tmpdir(), "fake-github-"))
  t.after(() => rmSync(dir, { recursive: true, force: true }))
  const app = JSON.parse(readFileSync(github.app(join(dir, "app.json")), "utf8"))
  const post = async (path, fields) =>
    (
      await fetch(`${app.webUrl}${path}`, {
        method: "POST",
        headers: { accept: "application/json" },
        body: new URLSearchParams(fields),
      })
    ).json()
  const refused = await post("/login/device/code", { client_id: "someone-else" })
  assert.equal(refused.error, "unauthorized_client")
  const code = await post("/login/device/code", { client_id: app.clientId })
  assert.match(code.user_code, /^[A-Z]{4}-[A-Z]{4}$/)
  assert.equal(code.user_code, github.userCode())
  assert.equal(code.verification_uri, github.verificationUri)
  const poll = () =>
    post("/login/oauth/access_token", {
      client_id: app.clientId,
      device_code: code.device_code,
      grant_type: "urn:ietf:params:oauth:grant-type:device_code",
    })
  assert.equal((await poll()).error, "authorization_pending")
  github.approve()
  assert.equal((await poll()).access_token, github.token)
  assert.equal((await poll()).error, "incorrect_device_code", "a code is used once")
  const user = (token) =>
    fetch(`${app.apiUrl}/user`, { headers: { authorization: `Bearer ${token}` } })
  assert.deepEqual(await (await user(github.token)).json(), FAKE_ACCOUNT)
  assert.equal((await user("ghu_other")).status, 401)
})
