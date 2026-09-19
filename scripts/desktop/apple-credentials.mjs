/**
 * Decides whether a build signs, notarizes, or does neither — and says so.
 *
 * The decision has to happen before the tool runs, because the tool cannot be
 * told "not configured" through the variables it reads. `tauri build` looks for
 * `APPLE_CERTIFICATE` with `var_os`, which answers "is this variable set", not
 * "does it have anything in it". A workflow that maps a missing secret into
 * `env:` sets the variable to the empty string, so the tool takes the import
 * branch, base64-decodes nothing, and runs `security import` on an empty file.
 * The build then fails, in a step whose whole point was to be dormant until the
 * secrets exist. There is no way to write an unset variable in a job's `env:`,
 * so the names the tool reads are never written there: they are promoted into
 * the environment here, only once there is something to put in them.
 *
 * It is also where a half-configured repository is caught. A certificate with
 * no password, or two of the three App Store Connect values, is somebody in the
 * middle of setting this up — and the failure it would otherwise cause arrives
 * late, inside the bundler, saying something about a keychain. Said here it
 * names the secret that is missing.
 */
import { randomUUID } from "node:crypto"
import { appendFileSync, writeFileSync } from "node:fs"
import { pathToFileURL } from "node:url"

/** A secret's value, or nothing. Empty and absent are the same thing here. */
function set(value) {
  const trimmed = value?.trim()
  return trimmed ? trimmed : undefined
}

function missing(wanted, values) {
  return wanted.filter((name) => !values[name])
}

/**
 * What this environment is configured to do, as the variables to export.
 *
 * @returns {{ signs: boolean, notarizes: boolean, exports: Record<string, string>, key?: string }}
 *   `exports` are the variables the tool reads, and are empty when nothing is
 *   configured. `key` is the App Store Connect key's own bytes, which the tool
 *   wants as a file rather than a value.
 * @throws when a credential set is partly there, naming what is missing.
 */
export function appleCredentials(environment, { keyPath } = {}) {
  const certificate = set(environment.SECRET_APPLE_CERTIFICATE)
  const password = set(environment.SECRET_APPLE_CERTIFICATE_PASSWORD)
  const identity = set(environment.SECRET_APPLE_SIGNING_IDENTITY)
  const keyId = set(environment.SECRET_APPLE_API_KEY_ID)
  const issuer = set(environment.SECRET_APPLE_API_ISSUER)
  const key = set(environment.SECRET_APPLE_API_KEY)

  // Named as the secrets a person sets, not as the variables the tool reads:
  // the answer to "which one is missing" has to be something you can go and
  // put in the repository's settings.
  const signing = {
    SECRET_APPLE_CERTIFICATE: certificate,
    SECRET_APPLE_CERTIFICATE_PASSWORD: password,
  }
  const signingMissing = missing(Object.keys(signing), signing)
  if (signingMissing.length === 1)
    throw new Error(
      `Signing is half configured: ${signingMissing[0]} is not set. Set it, or unset the other to build unsigned.`,
    )

  const notarizing = {
    SECRET_APPLE_API_KEY_ID: keyId,
    SECRET_APPLE_API_ISSUER: issuer,
    SECRET_APPLE_API_KEY: key,
  }
  const notarizingMissing = missing(Object.keys(notarizing), notarizing)
  if (notarizingMissing.length > 0 && notarizingMissing.length < 3)
    throw new Error(
      `Notarization is half configured: ${notarizingMissing.join(", ")} not set. ` +
        "All three of the App Store Connect key, its id and its issuer are needed, or none of them.",
    )

  const signs = signingMissing.length === 0
  const notarizes = notarizingMissing.length === 0

  // Notarizing an ad-hoc signature is not a thing Apple will do, and the tool's
  // failure when asked says nothing about the cause.
  if (notarizes && !signs)
    throw new Error(
      "The App Store Connect key is set but the Developer ID certificate is not. Apple notarizes signed bundles only.",
    )

  if (!signs) {
    // The identity alone names a certificate already in a keychain, which is how
    // a developer's own machine signs. A runner has no keychain of its own, so
    // it is only meaningful here alongside one of the two paths above.
    return { signs: false, notarizes: false, exports: {} }
  }

  const exports = { APPLE_CERTIFICATE: certificate, APPLE_CERTIFICATE_PASSWORD: password }
  if (identity) exports.APPLE_SIGNING_IDENTITY = identity
  if (notarizes) {
    if (!keyPath) throw new Error("Notarizing needs a path to write the key to")
    exports.APPLE_API_KEY = keyId
    exports.APPLE_API_ISSUER = issuer
    exports.APPLE_API_KEY_PATH = keyPath
  }
  return { signs, notarizes, exports, key: notarizes ? key : undefined }
}

/**
 * Promotes what is configured into the job's environment, and writes the key.
 *
 * `GITHUB_ENV` is the only way a step can set a variable for the steps after
 * it, and it is the reason this runs as a step at all rather than as job-level
 * `env:` — a variable not written there is genuinely unset for the build.
 *
 * The heredoc form is used for every value because one of them is a PEM and one
 * is base64 that may wrap; a `NAME=value` line cannot carry a newline. The
 * delimiter is random so a value cannot contain it, which is the documented way
 * to smuggle arbitrary variables into a job.
 */
function main() {
  const keyPath = process.env.APPLE_API_KEY_FILE
  const { signs, notarizes, exports, key } = appleCredentials(process.env, { keyPath })

  const lines = Object.entries(exports).map(([name, value]) => {
    const delimiter = `ghenv-${randomUUID()}`
    return `${name}<<${delimiter}\n${value}\n${delimiter}`
  })
  if (lines.length > 0) appendFileSync(process.env.GITHUB_ENV, `${lines.join("\n")}\n`)
  if (key) writeFileSync(keyPath, key, { mode: 0o600 })

  // Said out loud, because "was this release signed" is the question asked of a
  // build log months later, and the answer is otherwise an absence.
  process.stdout.write(
    signs
      ? `→ signing with a Developer ID certificate; ${notarizes ? "notarizing and stapling" : "not notarizing (no App Store Connect key)"}\n`
      : "→ no Apple credentials: signing ad-hoc, which Gatekeeper refuses anywhere but the machine that built it\n",
  )
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main()
