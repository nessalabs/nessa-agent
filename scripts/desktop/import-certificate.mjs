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
 * The Developer ID Application identity in `security find-identity` output, as
 * the SHA-1 hash that names it unambiguously.
 *
 * The hash rather than the human-readable name because the name is what a
 * person reads and the hash is what `codesign` resolves without guessing: two
 * certificates for the same team differ by hash, not by name.
 *
 * Deliberately only "Developer ID Application". A keychain may also hold a
 * "Developer ID Installer" certificate, which signs installer packages and
 * cannot sign an executable — picking the first line would sometimes work and
 * sometimes produce a failure deep inside the bundler.
 *
 * @param {string} output `security find-identity -v -p codesigning` output
 * @returns {string} the identity's SHA-1 hash
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
  return found[0][1]
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

  try {
    writeFileSync(pkcs12, Buffer.from(certificate, "base64"), { mode: 0o600 })
    execFileSync("security", ["create-keychain", "-p", unlock, keychain])
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

    const identity =
      process.env.APPLE_SIGNING_IDENTITY?.trim() ||
      developerIdIdentity(
        execFileSync("security", ["find-identity", "-v", "-p", "codesigning", keychain], {
          encoding: "utf8",
        }),
      )
    exportVariable("APPLE_SIGNING_IDENTITY", identity)
    // Named so the cleanup step can delete it whatever the build did.
    exportVariable("NESSA_SIGNING_KEYCHAIN", keychain)
    process.stdout.write(
      `→ imported a Developer ID certificate; signing with ${identity}\n`,
    )
  } finally {
    // The PKCS#12 and its password are the certificate itself. The keychain
    // has what the build needs; this file has no further use and a private key
    // on a runner's disk outlives its usefulness the moment it is imported.
    rmSync(scratch, { recursive: true, force: true })
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main()
