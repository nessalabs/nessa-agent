import assert from "node:assert/strict"
import { test } from "node:test"
import { appleCredentials } from "./apple-credentials.mjs"

const KEY_PATH = "/work/.apple-api-key.p8"
const credentials = (environment) => appleCredentials(environment, { keyPath: KEY_PATH })

const CERTIFICATE = {
  SECRET_APPLE_CERTIFICATE: "base64-pkcs12",
  SECRET_APPLE_CERTIFICATE_PASSWORD: "hunter2",
}
const KEY = {
  SECRET_APPLE_API_KEY_ID: "ABCDE12345",
  SECRET_APPLE_API_ISSUER: "1234-5678",
  SECRET_APPLE_API_KEY: "-----BEGIN PRIVATE KEY-----\nxx\n-----END PRIVATE KEY-----",
}

/**
 * A repository with no secrets exports nothing at all.
 *
 * Not "exports them empty": the bundler asks whether the variable is set, not
 * whether it has anything in it, so an empty `APPLE_CERTIFICATE` sends it to
 * import a certificate that is not there. Nothing exported is the only way to
 * say "not configured" to it.
 */
test("no secrets configured exports nothing", () => {
  assert.deepEqual(credentials({}), { signs: false, notarizes: false, exports: {} })
})

/** A secret that exists but is empty is a secret that is not set. */
test("empty secrets are the same as absent ones", () => {
  const result = credentials({
    SECRET_APPLE_CERTIFICATE: "",
    SECRET_APPLE_CERTIFICATE_PASSWORD: "  ",
    SECRET_APPLE_API_KEY_ID: "",
    SECRET_APPLE_API_ISSUER: "",
    SECRET_APPLE_API_KEY: "",
  })
  assert.deepEqual(result, { signs: false, notarizes: false, exports: {} })
})

test("a certificate and its password sign, and nothing notarizes", () => {
  const { signs, notarizes, exports, key } = credentials({ ...CERTIFICATE })
  assert.equal(signs, true)
  assert.equal(notarizes, false)
  assert.deepEqual(exports, {
    APPLE_CERTIFICATE: "base64-pkcs12",
    APPLE_CERTIFICATE_PASSWORD: "hunter2",
  })
  assert.equal(key, undefined)
  assert.ok(
    !("APPLE_API_KEY" in exports),
    "no key means the bundler is never asked to notarize",
  )
})

test("a full set signs, notarizes, and hands the key over as a file", () => {
  const { signs, notarizes, exports, key } = credentials({ ...CERTIFICATE, ...KEY })
  assert.equal(signs, true)
  assert.equal(notarizes, true)
  assert.deepEqual(exports, {
    APPLE_CERTIFICATE: "base64-pkcs12",
    APPLE_CERTIFICATE_PASSWORD: "hunter2",
    APPLE_API_KEY: "ABCDE12345",
    APPLE_API_ISSUER: "1234-5678",
    APPLE_API_KEY_PATH: KEY_PATH,
  })
  assert.equal(
    key,
    KEY.SECRET_APPLE_API_KEY,
    "the key's own bytes go to a file, not a variable",
  )
  assert.ok(
    !("APPLE_API_KEY_P8" in exports),
    "the name we carry the key under is ours, not the tool's",
  )
})

test("a named identity rides along with a certificate", () => {
  const { exports } = credentials({
    ...CERTIFICATE,
    SECRET_APPLE_SIGNING_IDENTITY: "Developer ID Application: Someone (TEAM)",
  })
  assert.equal(exports.APPLE_SIGNING_IDENTITY, "Developer ID Application: Someone (TEAM)")
})

/**
 * Half-configured sets are the ones worth a sentence. Each of these otherwise
 * fails deep inside the bundler, saying something about a keychain or a
 * notarization request, at the end of a twenty-minute build.
 */
test("a certificate with no password is refused by name", () => {
  assert.throws(
    () => credentials({ SECRET_APPLE_CERTIFICATE: "base64-pkcs12" }),
    /SECRET_APPLE_CERTIFICATE_PASSWORD is not set/,
  )
})

test("a password with no certificate is refused by name", () => {
  assert.throws(
    () => credentials({ SECRET_APPLE_CERTIFICATE_PASSWORD: "hunter2" }),
    /SECRET_APPLE_CERTIFICATE is not set/,
  )
})

test("two of the three App Store Connect values are refused by name", () => {
  assert.throws(
    () =>
      credentials({
        ...CERTIFICATE,
        SECRET_APPLE_API_KEY_ID: "ABCDE12345",
        SECRET_APPLE_API_ISSUER: "1234-5678",
      }),
    /SECRET_APPLE_API_KEY not set/,
  )
})

test("one of the three is refused too, naming both that are missing", () => {
  assert.throws(
    () => credentials({ ...CERTIFICATE, SECRET_APPLE_API_KEY_ID: "ABCDE12345" }),
    /SECRET_APPLE_API_ISSUER, SECRET_APPLE_API_KEY not set/,
  )
})

/** Apple notarizes signed bundles. Asking it to notarize an ad-hoc one is a
 * mistake worth naming here rather than at the submission. */
test("a key with no certificate is refused", () => {
  assert.throws(() => credentials({ ...KEY }), /Apple notarizes signed bundles only/)
})

/** Notarizing needs somewhere to put the key, and a caller that forgot is a bug
 * here, not a build that quietly does not notarize. */
test("notarizing without a path to write the key to is refused", () => {
  assert.throws(
    () => appleCredentials({ ...CERTIFICATE, ...KEY }, {}),
    /needs a path to write the key to/,
  )
})
