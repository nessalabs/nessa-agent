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
import { randomBytes } from "node:crypto"
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
export async function startFakeGithub() {
  const token = `ghu_${randomBytes(18).toString("hex")}`
  const codes = new Map()
  let latest = null
  const calls = { device: 0, poll: 0, user: 0, repository: 0 }
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
        if (form.get("client_id") !== CLIENT_ID)
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
          form.get("client_id") !== CLIENT_ID ||
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
      if (request.method === "GET" && url.pathname === "/repos/nessalabs/nessa-agent") {
        calls.repository += 1
        if (request.headers.authorization !== `Bearer ${token}`)
          return json(response, 401, { message: "Bad credentials" })
        return json(response, 200, { full_name: "nessalabs/nessa-agent" })
      }
      json(response, 404, { message: "Not Found" })
    })
  })
  await new Promise((listening) => server.listen(0, "127.0.0.1", listening))
  const base = `http://127.0.0.1:${server.address().port}`
  return {
    token,
    calls,
    /** The code the last device-code call handed out, or null. */
    userCode: () => (latest ? codes.get(latest).userCode : null),
    verificationUri: `${base}/login/device`,
    installUrl: `${base}/apps/${SLUG}/installations/new`,
    /** The person enters the last code on GitHub and approves the app. */
    approve() {
      if (!latest) throw new Error("no code was asked for")
      codes.get(latest).approved = true
    },
    /** Writes the app description the gateway reads from `NESSA_GITHUB_APP`. */
    app(file) {
      writeFileSync(
        file,
        JSON.stringify({ clientId: CLIENT_ID, slug: SLUG, webUrl: base, apiUrl: base }),
      )
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
