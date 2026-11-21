/**
 * OpenAI's sign-in for agents, on this machine, for the stress check's
 * `chatgpt` step: the calls the gateway makes
 * (`crates/nessa-gateway/src/logins/chatgpt`) answered as OpenAI documents
 * them, with the person's side — approving Nessa in the browser — done by
 * `approve(authorizeUrl, account)`, which plays the browser.
 *
 * ```text
 *   GET  /authorize   the browser: back to the gateway's loopback with a code,
 *                     the state, and the client id issued to the account
 *   POST /token       authorization_code once per code (PKCE checked), then
 *                     refresh_token once per refresh token (rotated; a reused
 *                     one is refused as refresh_token_reused)
 *   POST /revoke      a refresh token, not usable after
 *   GET  /jwks        the key the ID tokens are signed with (RS256)
 * ```
 *
 * `config(file)` writes the description the gateway reads from
 * `NESSA_CHATGPT_SIGN_IN`. Access tokens are JWT-shaped, as OpenAI's are, and
 * last `expiresIn` seconds: under the gateway's refresh margin, so every
 * hand-out refreshes and every lease is renewed a minute after it starts.
 */
import { createHash, generateKeyPairSync, randomBytes, sign } from "node:crypto"
import { createServer } from "node:http"
import { writeFileSync } from "node:fs"

/** The two ChatGPT accounts the fake signs in. */
export const FAKE_CHATGPT_ACCOUNTS = [
  { subject: "user-stress-a", email: "stress-a@example.test" },
  { subject: "user-stress-b", email: "stress-b@example.test" },
]
const REGISTRATION_CLIENT = "dynamic_agent_client"
const PLAN_SCOPE = "chatgpt.tokens.use.direct"

const base64url = (value) =>
  Buffer.from(typeof value === "string" ? value : JSON.stringify(value)).toString(
    "base64url",
  )

/** Starts the fake on a free loopback port. Resolves with its controls. */
export async function startFakeOpenai({ expiresIn = 11 * 60 } = {}) {
  const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 })
  const kid = "stress-key"
  const jwk = { ...publicKey.export({ format: "jwk" }), kid, alg: "RS256", use: "sig" }
  // What each code, client, and token stands for.
  const codes = new Map()
  const clients = new Map()
  const refreshes = new Map()
  /** Every access token handed out, in order: `{ token, subject }`. */
  const issued = []
  const calls = { authorize: 0, exchange: 0, refresh: 0, reused: 0, revoke: 0, jwks: 0 }
  let base = ""
  const json = (response, status, body) => {
    response.writeHead(status, { "content-type": "application/json" })
    response.end(JSON.stringify(body))
  }
  const accessToken = (subject) => {
    const token = [
      base64url({ alg: "RS256", typ: "JWT", kid }),
      base64url({ sub: subject, n: randomBytes(6).toString("hex") }),
      randomBytes(32).toString("base64url"),
    ].join(".")
    issued.push({ token, subject })
    return token
  }
  const idToken = (account, clientId, nonce) => {
    const head = base64url({ alg: "RS256", typ: "JWT", kid })
    const now = Math.floor(Date.now() / 1000)
    const claims = base64url({
      iss: base,
      aud: clientId,
      sub: account.subject,
      email: account.email,
      nonce,
      iat: now,
      exp: now + 3600,
    })
    const signature = sign("sha256", Buffer.from(`${head}.${claims}`), privateKey)
    return `${head}.${claims}.${signature.toString("base64url")}`
  }
  const tokens = (account, clientId, nonce) => {
    const refresh = `rt_${randomBytes(24).toString("hex")}`
    refreshes.set(refresh, { account, clientId, used: false })
    return {
      access_token: accessToken(account.subject),
      refresh_token: refresh,
      id_token: idToken(account, clientId, nonce),
      token_type: "Bearer",
      expires_in: expiresIn,
      scope: `openid profile email offline_access resource.invoke ${PLAN_SCOPE}`,
    }
  }
  const server = createServer((request, response) => {
    let body = ""
    request.on("data", (chunk) => (body += chunk))
    request.on("end", () => {
      const form = new URLSearchParams(body)
      const url = new URL(request.url ?? "/", "http://fake")
      if (request.method === "GET" && url.pathname === "/authorize") {
        // Only `approve` reaches here, naming the account it signs in as.
        calls.authorize += 1
        const query = url.searchParams
        const account = FAKE_CHATGPT_ACCOUNTS.find(
          (each) => each.subject === query.get("stress_account"),
        )
        const redirect = query.get("redirect_uri") ?? ""
        if (
          !account ||
          !redirect.startsWith("http://127.0.0.1:") ||
          query.get("response_type") !== "code" ||
          query.get("code_challenge_method") !== "S256" ||
          !query.get("scope")?.split(" ").includes(PLAN_SCOPE)
        )
          return json(response, 400, { error: "invalid_request" })
        // A first sign-in registers through the shared client and is issued
        // one of its own; a sign-in again comes with the one it was issued.
        let clientId = query.get("client_id") ?? ""
        if (clientId === REGISTRATION_CLIENT)
          clientId = `oaiapp_${randomBytes(8).toString("hex")}`
        clients.set(clientId, account)
        const code = randomBytes(16).toString("hex")
        codes.set(code, {
          account,
          clientId,
          redirect,
          nonce: query.get("nonce"),
          challenge: query.get("code_challenge"),
        })
        const back = new URL(redirect)
        back.searchParams.set("code", code)
        back.searchParams.set("state", query.get("state") ?? "")
        back.searchParams.set("client_id", clientId)
        response.writeHead(302, { location: back.toString() })
        return response.end()
      }
      if (request.method === "POST" && url.pathname === "/token") {
        if (form.get("grant_type") === "authorization_code") {
          calls.exchange += 1
          const code = codes.get(form.get("code") ?? "")
          codes.delete(form.get("code") ?? "")
          const verifier = form.get("code_verifier") ?? ""
          const challenge = createHash("sha256").update(verifier).digest("base64url")
          if (
            !code ||
            code.clientId !== form.get("client_id") ||
            code.redirect !== form.get("redirect_uri") ||
            code.challenge !== challenge
          )
            return json(response, 400, { error: "invalid_grant" })
          return json(response, 200, tokens(code.account, code.clientId, code.nonce))
        }
        if (form.get("grant_type") === "refresh_token") {
          calls.refresh += 1
          const held = refreshes.get(form.get("refresh_token") ?? "")
          if (!held || held.clientId !== form.get("client_id"))
            return json(response, 400, { error: "invalid_grant" })
          if (held.used) {
            calls.reused += 1
            return json(response, 400, { error: "refresh_token_reused" })
          }
          held.used = true
          return json(response, 200, tokens(held.account, held.clientId, null))
        }
        return json(response, 400, { error: "unsupported_grant_type" })
      }
      if (request.method === "POST" && url.pathname === "/revoke") {
        calls.revoke += 1
        const held = refreshes.get(form.get("token") ?? "")
        if (held) held.used = true
        return json(response, 200, {})
      }
      if (request.method === "GET" && url.pathname === "/jwks") {
        calls.jwks += 1
        return json(response, 200, { keys: [jwk] })
      }
      json(response, 404, { error: "not_found" })
    })
  })
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve))
  base = `http://127.0.0.1:${server.address().port}`
  return {
    calls,
    issued,
    /** The account an access token was handed out for, if this fake made it. */
    subjectOf: (token) => issued.find((each) => each.token === token)?.subject,
    /** Every token this fake handed out, to look for where none should be. */
    secrets: () => [...issued.map((each) => each.token), ...refreshes.keys()],
    /** Writes the gateway's `NESSA_CHATGPT_SIGN_IN` file; answers its path. */
    config(file) {
      writeFileSync(
        file,
        JSON.stringify({
          issuer: base,
          authorizeUrl: `${base}/authorize`,
          tokenUrl: `${base}/token`,
          revokeUrl: `${base}/revoke`,
          jwksUrl: `${base}/jwks`,
        }),
      )
      return file
    },
    /**
     * The browser: open `authorizeUrl` as `account` and follow it back to
     * the gateway. Answers what the gateway's loopback said.
     */
    async approve(authorizeUrl, account) {
      const url = new URL(authorizeUrl)
      if (url.origin !== base) throw new Error(`not this fake's address: ${url.origin}`)
      url.searchParams.set("stress_account", account.subject)
      const answer = await fetch(url, { redirect: "follow" })
      return { status: answer.status, text: await answer.text() }
    },
    close: () => new Promise((resolve) => server.close(() => resolve())),
  }
}
