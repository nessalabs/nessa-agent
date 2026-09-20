/**
 * Puts the Developer ID certificate where `codesign` can find it, early.
 *
 * The bundler imports the certificate itself, into a temporary keychain it
 * makes and throws away — but it does that when it signs the app, which is
 * after `beforeBuildCommand` has already assembled and signed the runtime tree.
 * So at the moment `prepare-macos.mjs` needs an identity there is no keychain
 * holding one, and the nested executables would go on being signed ad-hoc no
 * matter how well the release is configured.
 *
 *   import-certificate ─▶ prepare-macos signs the runtime ─▶ tauri signs the app
 *        (this file)          (needs an identity here)         (imports its own)
 *
 * Both imports are deliberate. This one is ours and is deleted by the workflow
 * step that always runs; the bundler's is its business and it cleans up after
 * itself. Neither touches a login keychain, so nothing here outlives the job.
 *
 * Dormant without credentials: a developer's build, and a repository that has
 * not been given secrets yet, both exit having done nothing and say so. The
 * runtime is then signed ad-hoc, exactly as it was before any of this existed.
 */
import { execFileSync } from "node:child_process"
import { randomUUID } from "node:crypto"
import { appendFileSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { pathToFileURL } from "node:url"

/**
 * The Developer ID Application identity in `security find-identity` output,
 * both ways it can be named.
 *
 * Both, because the two consumers disagree about what an identity is. Our own
 * `codesign` calls take the SHA-1 hash, which resolves to one certificate and
 * cannot be ambiguous. The bundler takes `APPLE_SIGNING_IDENTITY` as the
 * human-readable *name* and checks it against the name on the certificate it
 * imported — a hash there matches nothing and fails the build before anything
 * is signed. Exporting the hash into that variable is the mistake this
 * function's shape exists to prevent.
 *
 * Deliberately only "Developer ID Application". A keychain may also hold a
 * "Developer ID Installer" certificate, which signs installer packages and
 * cannot sign an executable — picking the first line would sometimes work and
 * sometimes produce a failure deep inside the bundler.
 *
 * @param {string} output `security find-identity -v -p codesigning` output
 * @returns {{ hash: string, name: string }}
 * @throws when there is no such identity, or more than one to choose between
 */
export function developerIdIdentity(output) {
  const found = [...output.matchAll(/^\s*\d+\)\s+([0-9A-F]{40})\s+"(.+)"\s*$/gim)].filter(
    ([, , name]) => name.startsWith("Developer ID Application:"),
  )
  if (found.length === 0)
    throw new Error(
      "The imported certificate is not a Developer ID Application certificate. " +
        "Apple notarizes bundles signed with one of those and no other kind.\n" +
        output,
    )
  if (found.length > 1)
    throw new Error(
      "More than one Developer ID Application certificate was imported. " +
        "Set APPLE_SIGNING_IDENTITY to name the one this release signs with.\n" +
        output,
    )
  const [, hash, name] = found[0]
  return { hash, name }
}

/** Writes a variable for every step after this one. */
function exportVariable(name, value) {
  const delimiter = `ghenv-${randomUUID()}`
  appendFileSync(
    process.env.GITHUB_ENV,
    `${name}<<${delimiter}\n${value}\n${delimiter}\n`,
  )
}

function main() {
  const certificate = process.env.APPLE_CERTIFICATE?.trim()
  const password = process.env.APPLE_CERTIFICATE_PASSWORD ?? ""
  if (!certificate) {
    // Not a failure. apple-credentials.mjs has already decided this build has
    // no credentials and said so; repeating the reasoning here would be a
    // second place for it to drift.
    process.stdout.write(
      "→ no certificate to import; the runtime will be signed ad-hoc\n",
    )
    return
  }

  // A password of its own, never the certificate's: this keychain exists for
  // one job and unlocking it is not a secret worth reusing.
  const unlock = randomUUID()
  const keychain = `nessa-signing-${process.pid}.keychain-db`
  const scratch = mkdtempSync(join(tmpdir(), "nessa-signing-"))
  const pkcs12 = join(scratch, "certificate.p12")

  execFileSync("security", ["create-keychain", "-p", unlock, keychain])
  // Before the private key goes in, not after the identity comes out. The
  // workflow's cleanup deletes what this variable names, so anything that can
  // fail between the two would otherwise leave an unlocked keychain holding a
  // Developer ID key on the runner, with nothing naming it to delete.
  exportVariable("NESSA_SIGNING_KEYCHAIN", keychain)

  try {
    writeFileSync(pkcs12, Buffer.from(certificate, "base64"), { mode: 0o600 })
    // Without this the keychain relocks on a timer partway through a long
    // build, and the signature that fails is the one at the end of it.
    execFileSync("security", ["set-keychain-settings", keychain])
    execFileSync("security", ["unlock-keychain", "-p", unlock, keychain])
    execFileSync("security", [
      "import",
      pkcs12,
      "-k",
      keychain,
      "-P",
      password,
      "-T",
      "/usr/bin/codesign",
    ])
    // Otherwise the first signature opens a GUI prompt for permission to use
    // the key, on a machine with nobody at it, and the build hangs until it
    // times out.
    execFileSync("security", [
      "set-key-partition-list",
      "-S",
      "apple-tool:,apple:,codesign:",
      "-s",
      "-k",
      unlock,
      keychain,
    ])
    // Prepended, not substituted: dropping the existing search list would take
    // the system roots with it, and a certificate whose chain cannot be built
    // is a certificate codesign will not use.
    const existing = [
      ...execFileSync("security", ["list-keychains", "-d", "user"])
        .toString()
        .matchAll(/"([^"]+)"/g),
    ].map(([, path]) => path)
    execFileSync("security", [
      "list-keychains",
      "-d",
      "user",
      "-s",
      keychain,
      ...existing,
    ])

    // Two names for one certificate, because the two consumers want different
    // ones. APPLE_SIGNING_IDENTITY is the bundler's, and it compares it with
    // the name on the certificate it imports for itself — a SHA-1 hash there
    // matches nothing and fails the build. The hash is ours, for the codesign
    // calls in prepare-macos.mjs, where it is the unambiguous way to say which
    // certificate. A name set in the repository's secrets is a person's
    // decision and is left exactly as it is, for both.
    // `|| undefined`, not `?.trim()` alone: a workflow that maps an unset
    // secret into `env:` sets this to the empty string, and an empty identity
    // is not an identity. Taking it as one exports nothing and signs with
    // nothing.
    const named = process.env.APPLE_SIGNING_IDENTITY?.trim() || undefined
    const found = named
      ? undefined
      : developerIdIdentity(
          execFileSync(
            "security",
            ["find-identity", "-v", "-p", "codesigning", keychain],
            { encoding: "utf8" },
          ),
        )
    const identity = named ?? found.name
    if (!named) exportVariable("APPLE_SIGNING_IDENTITY", identity)
    exportVariable("NESSA_RUNTIME_SIGNING_IDENTITY", found?.hash ?? named)
    process.stdout.write(
      `→ imported a Developer ID certificate; signing with ${identity}\n`,
    )
  } catch (failure) {
    // The keychain holds a private key and this build is not going to use it.
    // The workflow would delete it anyway; doing it here as well means a
    // failure does not depend on a later step running at all. The original
    // error is what gets raised — a cleanup that also fails must not replace
    // the reason the build stopped.
    try {
      execFileSync("security", ["delete-keychain", keychain])
    } catch {
      process.stderr.write(
        `Could not delete ${keychain} after a failed import; the workflow's cleanup step will try again.\n`,
      )
    }
    throw failure
  } finally {
    // The PKCS#12 and its password are the certificate itself. The keychain
    // has what the build needs; this file has no further use and a private key
    // on a runner's disk outlives its usefulness the moment it is imported.
    rmSync(scratch, { recursive: true, force: true })
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main()
