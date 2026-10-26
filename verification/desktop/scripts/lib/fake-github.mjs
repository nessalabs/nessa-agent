/**
 * GitHub's device flow, on this machine, for the stress check's `github`
 * step: the three calls the gateway makes (`crates/nessa-server/src/github`)
 * answered as GitHub documents them, with the person's side — entering the
 * code on github.com — done by `approve()`.
 *
 * ```text
 *   POST /login/device/code         a device code and the user code to show
 *   POST /login/oauth/access_token  authorization_pending until approve(),
 *                                   then the token, once per code
 *   GET  /user                      the account, for that token only
 * ```
 *
 * `app(file)` writes the app description the gateway reads from
 * `NESSA_GITHUB_APP`, naming this server for both GitHub's web and its API.
 * Each code is a new one and the last is the one approved; a token is only
 * ever handed out for an approved code, and `token` is the one it hands out.
 */
import { homedir } from "node:os"
import { spawnSync } from "node:child_process"
import { join, basename } from "node:path"
import { createHash, randomBytes } from "node:crypto"
import { createServer } from "node:http"
import { writeFileSync } from "node:fs"

/** The account the fake signs in. */
export const FAKE_ACCOUNT = { id: 4242, login: "stress-octo" }
/**
 * Where the window asks for an account's picture (the desktop's
 * `githubAvatarUrl`), and the picture the page is answered with there: a
 * 56px grey square, so nothing leaves the machine.
 */
export const GITHUB_AVATARS = "https://avatars.githubusercontent.com/u"
export const FAKE_AVATAR = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAADgAAAA4CAIAAAAn5KxJAAAAP0lEQVR42u3OMQ0AAAwDoEqv9B5TsQQUkD4RUVFRUVFRUVFRUVFRUVFRUVFRUVFRUVFRUVFRUVFRUVFRUVHRM4zeYQ8NTLd/AAAAAElFTkSuQmCC",
  "base64",
)
/** The fake app's identity, as the released app file names it. */
const CLIENT_ID = "Iv1.stressfake"
const SLUG = "nessa-stress"

/** Starts the fake on a free loopback port. Resolves with its controls. */
export async function startFakeGithub({ clientId = CLIENT_ID, slug = SLUG } = {}) {
  const token = `ghu_${randomBytes(18).toString("hex")}`
  const codes = new Map()
  let latest = null
  let repositoryCount = 1
  let repositoryFailure = false
  const calls = { device: 0, poll: 0, user: 0 }
  const json = (response, status, body) => {
    response.writeHead(status, { "content-type": "application/json" })
    response.end(JSON.stringify(body))
  }
  const server = createServer((request, response) => {
    let body = ""
    request.on("data", (chunk) => (body += chunk))
    request.on("end", () => {
      const form = new URLSearchParams(body)
      const url = new URL(request.url ?? "/", "http://fake")
      if (request.method === "POST" && url.pathname === "/login/device/code") {
        calls.device += 1
        if (form.get("client_id") !== clientId)
          return json(response, 200, { error: "unauthorized_client" })
        const deviceCode = randomBytes(12).toString("hex")
        const userCode = `${letters(4)}-${letters(4)}`
        codes.set(deviceCode, { userCode, approved: false, taken: false })
        latest = deviceCode
        return json(response, 200, {
          device_code: deviceCode,
          user_code: userCode,
          verification_uri: `${base}/login/device`,
          expires_in: 900,
          interval: 1,
        })
      }
      if (request.method === "POST" && url.pathname === "/login/oauth/access_token") {
        calls.poll += 1
        const code = codes.get(form.get("device_code") ?? "")
        if (
          form.get("client_id") !== clientId ||
          form.get("grant_type") !== "urn:ietf:params:oauth:grant-type:device_code" ||
          !code ||
          code.taken
        )
          return json(response, 200, { error: "incorrect_device_code" })
        if (!code.approved) return json(response, 200, { error: "authorization_pending" })
        code.taken = true
        return json(response, 200, {
          access_token: token,
          token_type: "bearer",
          scope: "",
        })
      }
      if (request.method === "GET" && url.pathname === "/user") {
        calls.user += 1
        if (request.headers.authorization !== `Bearer ${token}`)
          return json(response, 401, { message: "Bad credentials" })
        return json(response, 200, FAKE_ACCOUNT)
      }
      if (request.method === "GET" && url.pathname.startsWith("/user/installations")) {
        if (request.headers.authorization !== `Bearer ${token}`)
          return json(response, 401, {})
        if (repositoryFailure) return json(response, 503, {})
        if (url.pathname === "/user/installations")
          return json(response, 200, {
            total_count: 1,
            installations: [{ id: 77, app_slug: slug, suspended_at: null }],
          })
        if (url.pathname === "/user/installations/77/repositories")
          return json(response, 200, {
            total_count: repositoryCount,
            repositories: Array.from({ length: repositoryCount }, (_, i) => ({
              id: 42 + i,
              full_name: `owner/repo${i}`,
            })),
          })
      }
      json(response, 404, { message: "Not Found" })
    })
  })
  await new Promise((listening) => server.listen(0, "127.0.0.1", listening))
  const base = `http://127.0.0.1:${server.address().port}`
  return {
    token,
    calls,
    repositories(count, failed = false) {
      repositoryCount = count
      repositoryFailure = failed
    },
    /** The code the last device-code call handed out, or null. */
    userCode: () => (latest ? codes.get(latest).userCode : null),
    verificationUri: `${base}/login/device`,
    installUrl: `${base}/apps/${slug}/installations/new`,
    /** The person enters the last code on GitHub and approves the app. */
    approve() {
      if (!latest) throw new Error("no code was asked for")
      codes.get(latest).approved = true
    },
    /** Writes the app description the gateway reads from `NESSA_GITHUB_APP`. */
    app(file) {
      writeFileSync(file, JSON.stringify({ clientId, slug, webUrl: base, apiUrl: base }))
      return file
    },
    close: () => new Promise((closed) => server.close(closed)),
  }
}

/** `n` letters of the kind GitHub's user codes are made of. */
function letters(n) {
  const alphabet = "BCDFGHJKLMNPQRSTVWXZ"
  return [...randomBytes(n)].map((byte) => alphabet[byte % alphabet.length]).join("")
}

/** macOS resolves the login keychain through the OS user's home. These checks
 * use the real OS boundary with namespace-scoped disposable entries. The only
 * provider programs started are hermetic scripted agents; both supported API
 * credential sources are placeholders, so provider sign-ins are not imported. */
export function nativeGithubEnvironment() {
  return {
    ...(process.platform === "darwin" ? { HOME: homedir() } : {}),
    ANTHROPIC_API_KEY: "signed-out-gateway-placeholder",
    OPENAI_API_KEY: "signed-out-gateway-placeholder",
  }
}

/** Remove only this fixture's disposable login-keychain entries after its gateway
 * stops. Linux CI owns an ephemeral Secret Service collection for the whole job. */
export function cleanupNativeGithub(gateway, stage, instance) {
  if (process.platform !== "darwin") return
  if (!basename(gateway.directory).startsWith("nessa-mcp-live-"))
    throw new Error("Not a disposable gateway")
  const namespace = join(gateway.directory, stage, "instances", instance)
  const service = `ai.nessalabs.nessa.${createHash("sha256").update(namespace).digest("hex")}`
  for (const purpose of ["github-sign-in", "github-inbox-key"]) {
    const result = spawnSync(
      "security",
      ["delete-generic-password", "-s", service, "-a", purpose],
      { stdio: "ignore", timeout: 10000 },
    )
    if (result.status !== 0 && result.status !== 44)
      throw new Error("Disposable OS credential cleanup failed")
  }
}
