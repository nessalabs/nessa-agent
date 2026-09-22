import { execFileSync, spawnSync } from "node:child_process"
import { existsSync, readFileSync } from "node:fs"
import { resolve } from "node:path"
import { bundleArchitecture, includesDiskImage } from "./bundle-architecture.mjs"
import { verifyRuntimeFingerprint } from "./runtime-fingerprint.mjs"
import { runtimeExecutables } from "./runtime-layout.mjs"
import { signingProblems } from "./runtime-signing.mjs"

const root = resolve(import.meta.dirname, "../..")
const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    cwd: root,
    encoding: "utf8",
  }),
)
const target = process.env.NESSA_BUILD_TARGET
const release = target
  ? resolve(metadata.target_directory, target, "release")
  : resolve(metadata.target_directory, "release")
const app = resolve(release, "bundle/macos/Nessa.app")
if (!existsSync(app)) throw new Error(`macOS application bundle is absent: ${app}`)
const config = JSON.parse(readFileSync(resolve(root, "src-tauri/tauri.conf.json")))

const runtime = resolve(app, "Contents/Resources/runtime")
verifyRuntimeFingerprint(runtime)

execFileSync("codesign", ["--verify", "--deep", "--strict", "--verbose=2", app], {
  stdio: "inherit",
})
/**
 * Whether this build was signed with a real identity rather than ad hoc.
 *
 * The CLI infers the identity from `APPLE_CERTIFICATE` when one is imported and
 * `APPLE_SIGNING_IDENTITY` is not set, so either is evidence of a real signature
 * and the absence of both means the "-" fallback.
 */
const signedForReal =
  (process.env.APPLE_SIGNING_IDENTITY?.trim() &&
    process.env.APPLE_SIGNING_IDENTITY.trim() !== "-") ||
  Boolean(process.env.APPLE_CERTIFICATE?.trim())

/**
 * Whether it was also submitted to Apple. Notarization needs the key as well as
 * the certificate, so a signed build is not necessarily a notarized one.
 */
const notarized = signedForReal && Boolean(process.env.APPLE_API_KEY?.trim())

if (signedForReal)
  execFileSync("spctl", ["--assess", "--type", "execute", "--verbose=2", app], {
    stdio: "inherit",
  })

/**
 * The nested runtime executables, checked the way Apple checks them.
 *
 * `codesign --verify --deep` above passes on an ad-hoc signature, and passed
 * on the one that failed v0.1.0's notarization: three binaries under
 * `Resources/runtime` with no Developer ID, no timestamp and no hardened
 * runtime. The bundler does not sign a resource, so nothing before this point
 * would have noticed. Only when the build claims a real signature — an ad-hoc
 * build is supposed to look like this.
 */
if (signedForReal) {
  const problems = Object.values(runtimeExecutables("darwin")).flatMap((name) => {
    // codesign writes the display to stderr and nothing to stdout, so this
    // cannot be an execFileSync like every other call here.
    const shown = spawnSync(
      "codesign",
      ["--display", "--verbose=2", resolve(runtime, name)],
      { encoding: "utf8" },
    )
    if (shown.status !== 0)
      throw new Error(`Could not read the signature of runtime/${name}:\n${shown.stderr}`)
    return signingProblems(name, shown.stderr)
  })
  if (problems.length > 0)
    throw new Error(
      `The bundled runtime is not signed for distribution:\n- ${problems.join("\n- ")}`,
    )
}

/**
 * A notarized build must carry its ticket.
 *
 * Stapling is what lets a machine that has never seen this app launch it
 * without asking Apple — the case that matters, because the first launch after
 * a download is often on a network that is not working yet. It is also the step
 * most easily lost: notarization can succeed and the staple fail, and nothing
 * about the bundle looks different afterwards. Asserted here rather than
 * assumed, and only when a key was supplied, so an unsigned or merely-signed
 * build is unaffected.
 */
if (notarized) {
  execFileSync("xcrun", ["stapler", "validate", app], { stdio: "inherit" })
  // What Gatekeeper actually does on first launch, which `spctl --assess`
  // above does not cover for a downloaded file.
  execFileSync("spctl", ["--assess", "--type", "install", "--verbose=2", app], {
    stdio: "inherit",
  })
}

if (includesDiskImage(process.env.NESSA_BUILD_BUNDLES, config.bundle.targets)) {
  const architecture = bundleArchitecture(target, process.arch)
  const dmg = resolve(
    release,
    `bundle/dmg/${config.productName}_${config.version}_${architecture}.dmg`,
  )
  if (!existsSync(dmg)) throw new Error(`macOS disk image is absent: ${dmg}`)
  execFileSync("hdiutil", ["verify", dmg], { stdio: "inherit" })
  if (signedForReal)
    execFileSync("codesign", ["--verify", "--strict", "--verbose=2", dmg], {
      stdio: "inherit",
    })
  // The disk image is what a person downloads, so its ticket is the one
  // Gatekeeper reads before the app inside has ever run.
  if (notarized) execFileSync("xcrun", ["stapler", "validate", dmg], { stdio: "inherit" })
}
