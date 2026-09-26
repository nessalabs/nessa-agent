/**
 * Notarizes and staples the disk image, which nothing else does.
 *
 * The bundler notarizes the `.app`, staples its ticket, and then builds the
 * `.dmg` around it and stops. That leaves the app fine and the disk image
 * without a ticket of its own — and the disk image is the artifact a person
 * downloads, so it is the one Gatekeeper inspects first, before anything has
 * been copied out of it. Without a stapled ticket that check is an online
 * lookup, and the machine that cannot reach Apple on first launch is exactly
 * the case stapling exists for.
 *
 *   tauri build ─▶ .app notarized + stapled ─▶ .dmg built ─┐
 *                                                          │
 *   this file ────────────────────────────────────────────▶├─▶ .dmg notarized
 *                                                          │   + stapled
 *   verify-macos-bundle.mjs ───────────────────────────────┴─▶ both validated
 *
 * Submitting a second time is not a waste: a ticket is issued for the artifact
 * submitted, and the disk image is a different artifact from the app inside it.
 * The submission is quick — Apple has already seen and accepted the contents.
 *
 * Dormant unless the build notarized: no App Store Connect key means the app
 * has no ticket either, and there is nothing here to be done about that.
 */
import { execFileSync } from "node:child_process"
import { existsSync, readFileSync } from "node:fs"
import { resolve } from "node:path"
import { pathToFileURL } from "node:url"
import { bundleArchitecture, includesBundle } from "./bundle-architecture.mjs"

/**
 * The `notarytool submit` arguments for a disk image.
 *
 * `--wait` because the next thing that happens is stapling, and a ticket that
 * does not exist yet cannot be stapled. Apple's own tool polls; doing it here
 * would be a worse version of the same loop.
 *
 * Keyed by the App Store Connect key, which is what the rest of the release
 * uses: the key's own file, its id, and the issuer. The three are what
 * `apple-credentials.mjs` exports under the names the bundler reads, and they
 * are read here rather than passed so that a build with no key is a build that
 * never calls this.
 *
 * @returns {string[]} arguments after `notarytool`
 */
export function submissionArguments(diskImage, { keyPath, keyId, issuer }) {
  const missing = Object.entries({ keyPath, keyId, issuer })
    .filter(([, value]) => !value?.trim())
    .map(([name]) => name)
  if (missing.length > 0)
    throw new Error(`Cannot notarize the disk image without: ${missing.join(", ")}`)
  return [
    "submit",
    diskImage,
    "--key",
    keyPath,
    "--key-id",
    keyId,
    "--issuer",
    issuer,
    "--wait",
  ]
}

/**
 * Whether this build produced a disk image that needs a ticket, and where.
 *
 * `undefined` for every reason not to: no key, so the app was not notarized
 * either; no disk image in the bundles this build selected; or a build that is
 * not macOS at all. The caller treats all three the same way — say so, do
 * nothing, exit zero — because none of them is a failure.
 */
export function diskImageToNotarize({ environment, root, architecture }) {
  if (!environment.APPLE_API_KEY?.trim()) return undefined
  const config = JSON.parse(readFileSync(resolve(root, "src-tauri/tauri.conf.json")))
  if (!includesBundle(environment.NESSA_BUILD_BUNDLES, config.bundle.targets, "dmg"))
    return undefined
  const target = environment.NESSA_BUILD_TARGET
  const metadata = JSON.parse(
    execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
      cwd: root,
      encoding: "utf8",
    }),
  )
  const release = target
    ? resolve(metadata.target_directory, target, "release")
    : resolve(metadata.target_directory, "release")
  return resolve(
    release,
    `bundle/dmg/${config.productName}_${config.version}_${bundleArchitecture(target, architecture)}.dmg`,
  )
}

function main() {
  const root = resolve(import.meta.dirname, "../..")
  const diskImage = diskImageToNotarize({
    environment: process.env,
    root,
    architecture: process.arch,
  })
  if (!diskImage) {
    process.stdout.write("→ no notarized disk image to staple\n")
    return
  }
  if (!existsSync(diskImage)) throw new Error(`macOS disk image is absent: ${diskImage}`)

  execFileSync(
    "xcrun",
    [
      "notarytool",
      ...submissionArguments(diskImage, {
        keyPath: process.env.APPLE_API_KEY_PATH,
        keyId: process.env.APPLE_API_KEY,
        issuer: process.env.APPLE_API_ISSUER,
      }),
    ],
    { stdio: "inherit" },
  )
  execFileSync("xcrun", ["stapler", "staple", diskImage], { stdio: "inherit" })
  process.stdout.write("→ disk image notarized and stapled\n")
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main()
