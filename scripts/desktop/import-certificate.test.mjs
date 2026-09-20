import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { developerIdIdentity } from "./import-certificate.mjs"

const NAME = "Developer ID Application: Nessa Labs (ABCDE12345)"
const HASH = "A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4"
const APPLICATION = `  1) ${HASH} "${NAME}"`
const INSTALLER =
  '  2) 0F1E2D3C4B5A69788796A5B4C3D2E1F009182736 "Developer ID Installer: Nessa Labs (ABCDE12345)"'
const found = (...lines) =>
  `${lines.join("\n")}\n     ${lines.length} valid identities found\n`

test("the Developer ID Application certificate is found by hash and by name", () => {
  assert.deepEqual(developerIdIdentity(found(APPLICATION)), { hash: HASH, name: NAME })
})

/**
 * An installer certificate cannot sign an executable. Taking the first line
 * would pick it whenever it sorts first, and the failure would arrive inside
 * the bundler rather than here.
 */
test("an installer certificate is not mistaken for a signing one", () => {
  assert.equal(developerIdIdentity(found(INSTALLER, APPLICATION)).hash, HASH)
  assert.throws(
    () => developerIdIdentity(found(INSTALLER)),
    /not a Developer ID Application/,
  )
})

test("an empty keychain says what is wrong with it", () => {
  assert.throws(
    () => developerIdIdentity("     0 valid identities found\n"),
    /Developer ID/,
  )
})

/** Guessing between two certificates is how a release gets signed by the wrong one. */
test("two signing certificates ask to be told which", () => {
  const second = APPLICATION.replace("1)", "2)").replace("A1B2", "B2A1")
  assert.throws(
    () => developerIdIdentity(found(APPLICATION, second)),
    /APPLE_SIGNING_IDENTITY/,
  )
})

/**
 * The script itself, run against a `security` that is a shell script.
 *
 * The parser above can be right while the script around it is wrong, and both
 * bugs this covers were exactly that: a hash exported into the bundler's
 * variable, and a keychain left on the runner because the name of it was
 * exported after the private key went in. Neither is visible from a unit test
 * of `developerIdIdentity`.
 *
 * @param {object} options
 * @param {string} [options.failAt] the `security` subcommand that should fail
 * @param {Record<string, string>} [options.environment] extra variables
 */
function runImport({ failAt, environment = {} } = {}) {
  const directory = mkdtempSync(join(tmpdir(), "nessa-import-test-"))
  const log = join(directory, "security.log")
  const githubEnv = join(directory, "github-env")
  writeFileSync(githubEnv, "")
  // Records every call, answers the two that are read, and fails on demand.
  writeFileSync(
    join(directory, "security"),
    `#!/bin/sh
echo "$@" >> ${JSON.stringify(log)}
case "$1" in
  ${failAt ? `${failAt}) echo "stubbed failure of $1" >&2; exit 1;;` : ""}
  list-keychains) [ "$2" = "-d" ] && [ "$#" = 3 ] && echo '    "/Users/runner/Library/Keychains/login.keychain-db"'; exit 0;;
  find-identity) printf '%s\\n     1 valid identities found\\n' ${JSON.stringify(APPLICATION)}; exit 0;;
esac
exit 0
`,
    { mode: 0o755 },
  )

  const result = spawnSync(process.execPath, ["scripts/desktop/import-certificate.mjs"], {
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${directory}:${process.env.PATH}`,
      GITHUB_ENV: githubEnv,
      APPLE_CERTIFICATE: Buffer.from("not really a pkcs12").toString("base64"),
      APPLE_CERTIFICATE_PASSWORD: "hunter2",
      APPLE_SIGNING_IDENTITY: "",
      NESSA_RUNTIME_SIGNING_IDENTITY: "",
      ...environment,
    },
  })

  const calls = existsSync(log)
    ? readFileSync(log, "utf8").split("\n").filter(Boolean)
    : []
  const exported = Object.fromEntries(
    [...readFileSync(githubEnv, "utf8").matchAll(/^(\w+)<<(\S+)\n([\s\S]*?)\n\2$/gm)].map(
      ([, name, , value]) => [name, value],
    ),
  )
  rmSync(directory, { recursive: true, force: true })
  return { ...result, calls, exported }
}

/**
 * The bundler is handed a name, never a hash.
 *
 * `APPLE_SIGNING_IDENTITY` is compared by the bundler against the name on the
 * certificate it imports for itself. A hash matches no name, so a release with
 * the certificate set and no explicit identity would fail before it signed
 * anything — the automatic path would never have produced a release at all.
 */
test("the exported identity is the certificate's name, and the hash goes elsewhere", () => {
  const { status, exported } = runImport()
  assert.equal(status, 0)
  assert.equal(exported.APPLE_SIGNING_IDENTITY, NAME)
  assert.ok(
    NAME.includes(exported.APPLE_SIGNING_IDENTITY),
    "the bundler checks that the certificate's name contains this value",
  )
  assert.equal(exported.NESSA_RUNTIME_SIGNING_IDENTITY, HASH)
})

/** A name a person put in the repository's secrets is their decision. */
test("an explicitly configured identity is left alone", () => {
  const { status, exported, calls } = runImport({
    environment: { APPLE_SIGNING_IDENTITY: NAME },
  })
  assert.equal(status, 0)
  assert.equal(exported.APPLE_SIGNING_IDENTITY, undefined, "nothing to re-export")
  assert.equal(exported.NESSA_RUNTIME_SIGNING_IDENTITY, NAME)
  assert.ok(
    !calls.some((call) => call.startsWith("find-identity")),
    "the keychain was searched although it had been told which certificate",
  )
})

/** The keychain is named for cleanup before there is a private key in it. */
test("the keychain is registered for cleanup before the key is imported", () => {
  const { exported, calls } = runImport()
  assert.match(exported.NESSA_SIGNING_KEYCHAIN, /^nessa-signing-\d+\.keychain-db$/)
  assert.ok(
    calls.findIndex((call) => call.startsWith("create-keychain")) <
      calls.findIndex((call) => call.startsWith("import")),
    "the key was imported before the keychain existed",
  )
})

/**
 * A failure after the private key is in the keychain deletes the keychain.
 *
 * Both of these used to leave it behind: the name was exported at the end, so
 * the workflow's `always()` cleanup had nothing to delete, and the script's own
 * `finally` removed only the PKCS#12 file.
 */
for (const failAt of ["set-key-partition-list", "find-identity"])
  test(`a failure at ${failAt} deletes the keychain and keeps its own error`, () => {
    const { status, stderr, calls, exported } = runImport({ failAt })
    assert.equal(status, 1)
    assert.ok(
      calls.some((call) => call.startsWith("delete-keychain")),
      `a keychain holding the signing key was left on the runner:\n${calls.join("\n")}`,
    )
    assert.ok(
      exported.NESSA_SIGNING_KEYCHAIN,
      "the workflow's cleanup step has nothing to delete either",
    )
    assert.match(stderr, failAt === "find-identity" ? /Developer ID|stubbed/ : /stubbed/)
  })

/** No certificate is not a failure, and touches no keychain. */
test("nothing configured imports nothing", () => {
  const { status, stdout, calls } = runImport({
    environment: { APPLE_CERTIFICATE: "" },
  })
  assert.equal(status, 0)
  assert.match(stdout, /signed ad-hoc/)
  assert.deepEqual(calls, [])
})
