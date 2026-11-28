/**
 * Boat, on this machine, for the stress check's `boatSignIn` step: the
 * OAuth calls Sign in with Boat makes (`crates/nessa-conversation/src/
 * workspaces/infrastructure/boat_oauth.rs`) and the account reads the
 * gateway's validation makes before saving, answered as Boat documents them
 * (https://docs.boat.dev/oauth-apps).
 *
 * ```text
 *   GET  /api/boat/oauth2/authorize  the person approves at once: 302 to the
 *                                    redirect_uri with code, state and iss
 *   POST /api/boat/oauth2/token      authorization_code (PKCE S256 checked,
 *                                    once per code) or refresh_token (each
 *                                    refresh token works once)
 *   POST /api/boat/oauth2/revoke     forget a refresh token
 *   GET  /api/v1/me                  the account, for a live access token
 *   GET  /api/v1/environments        the `nessa` environment, in the one
 *                                    posture Nessa admits
 * ```
 *
 * `app(file)` writes the app description the gateway reads from
 * `NESSA_BOAT_APP`. `deny()` makes the next approval a Cancel.
 */
import { createHash, randomBytes } from "node:crypto"
import { createServer } from "node:http"
import { writeFileSync } from "node:fs"

/** The account the fake signs in. */
export const FAKE_BOAT_LOGIN = "stress-boater"
const CLIENT_ID = "boatapp_stressfake"

/** The `nessa` environment exactly as Boat lists one Nessa admits. */
function environment() {
  return {
    id: "11111111-1111-4111-8111-111111111111",
    name: "nessa",
    isDefault: false,
    latestVersionId: "22222222-2222-4222-8222-222222222222",
    latestVersionNumber: 2,
    safeForThirdParties: false,
    passGithub: false,
    passSecrets: false,
    passSandboxCredentials: false,
    passAgentsCredentials: true,
    credentialProfileId: null,
    proxyAiCredentials: false,
    access: { restricted: false, members: [] },
    effective: {
      passSecrets: false,
      passGithub: false,
      passSandboxCredentials: false,
      passAgentsCredentials: true,
    },
    envContents: "",
    secretFiles: [],
    selectedRepositories: [],
    safetyAnsweredAt: null,
    network: null,
    versions: [
      {
        id: "22222222-2222-4222-8222-222222222222",
        versionNumber: 2,
        sandboxCount: 0,
        createdAt: "2026-10-27T00:00:00Z",
      },
    ],
  }
}

/** Starts the fake on a free loopback port. Resolves with its controls. */
export async function startFakeBoat({ clientId = CLIENT_ID, expiresIn = 3600 } = {}) {
  const codes = new Map()
  const access = new Set()
  const refresh = new Set()
  const issued = []
  const calls = {
    authorize: 0,
    exchange: 0,
    refresh: 0,
    revoke: 0,
    me: 0,
    environments: 0,
  }
  let denying = false
  let lastAuthorize = null
  const json = (response, status, body) => {
    response.writeHead(status, { "content-type": "application/json" })
    response.end(JSON.stringify(body))
  }
  const issue = () => {
    const tokens = {
      access_token: `boat_${randomBytes(12).toString("hex")}`,
      token_type: "Bearer",
      expires_in: expiresIn,
      refresh_token: `boatrt_${randomBytes(12).toString("hex")}`,
      scope: "account.read",
    }
    access.add(tokens.access_token)
    refresh.add(tokens.refresh_token)
    issued.push(tokens.access_token, tokens.refresh_token)
    return tokens
  }
  const server = createServer((request, response) => {
    let body = ""
    request.on("data", (chunk) => (body += chunk))
    request.on("end", () => {
      const form = new URLSearchParams(body)
      const url = new URL(request.url ?? "/", base)
      const bearer = (request.headers.authorization ?? "").replace(/^Bearer /, "")
      if (request.method === "GET" && url.pathname === "/api/boat/oauth2/authorize") {
        calls.authorize += 1
        const q = url.searchParams
        lastAuthorize = Object.fromEntries(q)
        const redirect = q.get("redirect_uri") ?? ""
        if (
          q.get("client_id") !== clientId ||
          q.get("response_type") !== "code" ||
          q.get("code_challenge_method") !== "S256" ||
          !/^http:\/\/localhost:\d+\/boat\/callback$/.test(redirect)
        )
          return json(response, 400, { error: "invalid_request" })
        const back = new URL(redirect)
        back.searchParams.set("state", q.get("state") ?? "")
        back.searchParams.set("iss", base)
        if (denying) {
          denying = false
          back.searchParams.set("error", "access_denied")
        } else {
          const code = randomBytes(12).toString("hex")
          codes.set(code, { challenge: q.get("code_challenge"), redirect, used: false })
          back.searchParams.set("code", code)
        }
        response.writeHead(302, { location: back.toString() })
        return response.end()
      }
      if (request.method === "POST" && url.pathname === "/api/boat/oauth2/token") {
        if (form.get("client_id") !== clientId)
          return json(response, 401, { error: "invalid_client" })
        if (form.get("grant_type") === "authorization_code") {
          calls.exchange += 1
          const code = codes.get(form.get("code") ?? "")
          const verifier = form.get("code_verifier") ?? ""
          const challenge = createHash("sha256").update(verifier).digest("base64url")
          if (
            !code ||
            code.used ||
            code.redirect !== form.get("redirect_uri") ||
            code.challenge !== challenge
          )
            return json(response, 400, { error: "invalid_grant" })
          code.used = true
          return json(response, 200, issue())
        }
        if (form.get("grant_type") === "refresh_token") {
          calls.refresh += 1
          const presented = form.get("refresh_token") ?? ""
          if (!refresh.delete(presented))
            return json(response, 400, { error: "invalid_grant" })
          return json(response, 200, issue())
        }
        return json(response, 400, { error: "unsupported_grant_type" })
      }
      if (request.method === "POST" && url.pathname === "/api/boat/oauth2/revoke") {
        calls.revoke += 1
        refresh.delete(form.get("token") ?? "")
        return json(response, 200, {})
      }
      if (url.pathname.startsWith("/api/v1/") && !access.has(bearer))
        return json(response, 401, { ok: false, error: { code: "unauthorized" } })
      if (request.method === "GET" && url.pathname === "/api/v1/me") {
        calls.me += 1
        return json(response, 200, {
          ok: true,
          type: "user.info",
          user: { id: "u_stress", login: FAKE_BOAT_LOGIN, email: null },
        })
      }
      if (request.method === "GET" && url.pathname === "/api/v1/environments") {
        calls.environments += 1
        return json(response, 200, {
          ok: true,
          type: "environments.list",
          environments: [environment()],
        })
      }
      json(response, 404, { ok: false, error: { code: "not_found" } })
    })
  })
  await new Promise((listening) => server.listen(0, "127.0.0.1", listening))
  const base = `http://127.0.0.1:${server.address().port}`
  return {
    calls,
    /** Where the fake answers: Boat's web and API origin. */
    base,
    /** Every token handed out, to look for in logs and pages. */
    issued,
    /** The query of the last authorize visit. */
    lastAuthorize: () => lastAuthorize,
    /** Whether a refresh token is still live (not revoked, not spent). */
    live: () => refresh.size,
    /** The next approval is a Cancel on Boat's page. */
    deny() {
      denying = true
    },
    /** Writes the app description the gateway reads from `NESSA_BOAT_APP`. */
    app(file) {
      writeFileSync(
        file,
        JSON.stringify({ clientId, webUrl: base, apiUrl: `${base}/api/v1` }),
      )
      return file
    },
    close: () => new Promise((closed) => server.close(closed)),
  }
}
