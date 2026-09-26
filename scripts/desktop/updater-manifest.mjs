/**
 * The parts of a release manifest a local harness has to get exactly right.
 *
 * Kept apart from `updater-harness.mjs` — which signs, serves, and prints — so
 * the details the plugin is fussy about can be tested without a key, a server,
 * or a built artifact. Every one of them is a silent failure when wrong: the
 * app reports no update rather than an error.
 */

import { linuxBundles } from "./bundle-architecture.mjs"

/** The `{os}-{arch}` key `tauri-plugin-updater` looks up in `platforms`.
 *
 * The plugin's own `updater_os`/`updater_arch` decide this, and they do not
 * spell things the way Node does: macOS is `darwin`, not `macos`, and the
 * architectures are Rust's. A key that looks right but is not makes the
 * manifest simply not apply, which reads as "already up to date". */
export function updaterTarget(platform, arch) {
  const os = { darwin: "darwin", linux: "linux", win32: "windows" }[platform]
  const cpu = {
    x64: "x86_64",
    arm64: "aarch64",
    ia32: "i686",
    arm: "armv7",
    riscv64: "riscv64",
  }[arch]
  if (!os) throw new Error(`No updater target for platform ${platform}`)
  if (!cpu) throw new Error(`No updater target for architecture ${arch}`)
  return `${os}-${cpu}`
}

/** The release manifest, in the shape the plugin's `RemoteRelease` reads.
 *
 * `version`, `notes` and `pub_date` describe the release; `platforms.<target>`
 * is what a machine downloads and the signature it is checked against.
 * `pub_date` must be RFC 3339 or the plugin rejects the whole manifest while
 * parsing it, before it ever looks at the platform.
 *
 * `platforms` is a map because one published manifest answers every machine
 * that asks: a real release builds Apple Silicon and Intel on separate runners
 * and puts both keys in one file. The local harness serves one. Same builder,
 * so there is exactly one place that knows this shape. */
export function releaseManifest({ version, notes, platforms, published }) {
  const targets = Object.keys(platforms)
  if (targets.length === 0)
    throw new Error("A release manifest with no platforms applies to nothing")
  for (const target of targets) {
    const { signature, url } = platforms[target]
    if (!signature) throw new Error(`No signature for ${target}`)
    if (!url) throw new Error(`No artifact URL for ${target}`)
  }
  return {
    version,
    notes,
    pub_date: published,
    platforms: Object.fromEntries(
      targets.map((target) => [
        target,
        { signature: platforms[target].signature, url: platforms[target].url },
      ]),
    ),
  }
}

/** The artifact name a check-only run announces and then does not have.
 *
 * It is named for what it is, because it shows up in two places a person will
 * read: the `url` in the served manifest, and the 404 logged if a click ever
 * asks for it. */
export const CHECK_ONLY_ARTIFACT = "check-only-has-no-artifact.tar.gz"

/** The `signature` field a check-only run puts in the manifest.
 *
 * The plugin reads this field while parsing and only *uses* it once bytes have
 * been downloaded, so a check succeeds with any string here. It says in plain
 * words what it is, so a manifest captured from this mode can never be mistaken
 * for one that was signed. */
export const CHECK_ONLY_SIGNATURE = Buffer.from(
  "check-only harness: not a signature, and never verified",
).toString("base64")

/** The manifest a check-only run serves: real shape, absent release.
 *
 * Everything the plugin's *check* reads is genuine — the version it compares,
 * the RFC 3339 date it parses, the `{os}-{arch}` key it looks up. Everything
 * the plugin's *install* would read is deliberately not: the URL points at
 * nothing and the signature is a sentence. */
export function checkOnlyManifest({ version, notes, target, origin, published }) {
  return releaseManifest({
    version,
    notes,
    platforms: {
      [target]: {
        signature: CHECK_ONLY_SIGNATURE,
        url: `${origin}/${CHECK_ONLY_ARTIFACT}`,
      },
    },
    published,
  })
}

/** Where `createUpdaterArtifacts` leaves the thing an update installs.
 *
 * Per platform, because the plugin installs a different kind of thing on each:
 * an archived app bundle on macOS, an AppImage on Linux, the NSIS installer on
 * Windows. Candidates, not a name — the caller serves the first that exists.
 *
 * Linux has two, and which one a build wrote depends on `createUpdaterArtifacts`:
 * `true` leaves the AppImage as it is, and `"v1Compatible"` also archives it.
 * The plugin installs either — it extracts the AppImage when the bytes are gzip
 * and writes them straight out when they are not — so the harness looks for
 * both rather than deciding which setting the build was made under. A release
 * publishes the AppImage as it is (`release-assets.mjs`), beside the `.deb`,
 * which this harness does not serve.
 */
export function defaultArtifacts(platform, version, targetDirectory = "target") {
  const { appimage } = linuxBundles("Nessa", version, "amd64")
  const artifacts = {
    darwin: ["macos/Nessa.app.tar.gz"],
    linux: [appimage, `${appimage}.tar.gz`],
    win32: [`nsis/Nessa_${version}_x64-setup.exe`],
  }[platform]
  if (!artifacts)
    throw new Error(`No updater artifact is bundled for platform ${platform}`)
  return artifacts.map((artifact) => `${targetDirectory}/release/bundle/${artifact}`)
}
