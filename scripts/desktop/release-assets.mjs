/**
 * What a release publishes, named once, and how several runners' builds become
 * one `latest.json`.
 *
 * Every target builds on its own runner. Apple Silicon and Intel cannot be
 * built together — `prepare-macos.mjs` refuses a target triple that is not the
 * host's and downloads Node for `process.arch`, so a universal binary is not
 * available to us — and Linux builds on Linux. Both macOS bundlers write the
 * identical name `Nessa.app.tar.gz`. A GitHub release has one flat namespace,
 * so uploading both as they are would silently leave one architecture pointing
 * at the other's bytes — which fails far away, as a rejected signature on a
 * user's machine.
 *
 *   macos-latest   ─▶ aarch64 bundle ─┐                 ┌─▶ Nessa_0.1.0_darwin-aarch64.app.tar.gz
 *   macos-15-intel ─▶ x86_64 bundle  ─┼─ stage (rename) ─┼─▶ Nessa_0.1.0_darwin-x86_64.app.tar.gz
 *   ubuntu-22.04   ─▶ .deb           ─┘                 └─▶ Nessa_0.1.0_amd64.deb
 *                                                                │
 *                                            manifest ◀──────────┘  (+ each .sig)
 *                                                │
 *                                          latest.json  ── one file, every key
 *
 * Linux publishes the `.deb` under `{os}-{arch}-deb`. The plugin updates each
 * install in its own format and looks for `{os}-{arch}-{installer}` before
 * `{os}-{arch}`, so the key names the format: an install of any other kind is
 * never offered a `.deb`.
 *
 * No AppImage is released. Its bundler (linuxdeploy) rewrites every ELF file
 * under `usr/lib`, the runtime's executables included, so the runtime no
 * longer matches its fingerprint and the Claude agent's self-contained binary
 * can be damaged; `verify-linux-bundle.mjs` refuses such an AppImage.
 *
 * `stage` runs on each build runner and renames that target's output to the
 * name it will be published under. `manifest` runs once afterwards, over every
 * staged set, and writes the `latest.json` the updater endpoint serves. It
 * refuses to write one that is missing a key: a manifest without one is not a
 * smaller release, it is a release part of the installed base can never update
 * from, and it fails silently as "no update available".
 *
 * The manifest shape itself is not defined here. `updater-manifest.mjs` owns
 * it, is used by the local harness too, and stays the only place that knows
 * what `tauri-plugin-updater` reads.
 *
 *   node scripts/desktop/release-assets.mjs stage --target aarch64-apple-darwin --into staging
 *   node scripts/desktop/release-assets.mjs manifest --directory assets \
 *     --tag v0.1.0 --repository nessalabs/nessa-agent
 */

import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { basename, resolve } from "node:path"
import { pathToFileURL } from "node:url"
import {
  bundleArchitecture,
  linuxBundleArchitecture,
  linuxBundles,
} from "./bundle-architecture.mjs"
import { option } from "./cli.mjs"
import { releaseManifest, updaterTarget } from "./updater-manifest.mjs"

/** Each target a release builds: the updater's platform, Node's arch, and the
 * `--bundles` its build row passes — the bundles `stagedAssets` publishes.
 *
 * A total table rather than a parse of the triple: a target is released only
 * when it is written here, and `universal-apple-darwin` is not, because we
 * cannot build it and a key for a bundle that does not exist is worse than an
 * error. */
const TARGET_PLATFORMS = {
  "aarch64-apple-darwin": { platform: "darwin", arch: "arm64", bundles: "app,dmg" },
  "x86_64-apple-darwin": { platform: "darwin", arch: "x64", bundles: "app,dmg" },
  "x86_64-unknown-linux-gnu": { platform: "linux", arch: "x64", bundles: "deb" },
}

/** The targets a release builds, in the order a manifest lists them. */
export const RELEASE_TARGETS = Object.keys(TARGET_PLATFORMS)

function targetPlatform(target) {
  if (!Object.hasOwn(TARGET_PLATFORMS, target))
    throw new Error(`No release is built for ${target}`)
  return TARGET_PLATFORMS[target]
}

/** The `{os}-{arch}` manifest key for a target triple.
 *
 * Routed through the same `updaterTarget` the harness uses rather than spelled
 * out again, so there is one answer to what the plugin calls an architecture. */
export function releaseTarget(target) {
  const { platform, arch } = targetPlatform(target)
  return updaterTarget(platform, arch)
}

/** The `--bundles` a release build of this target passes: what it publishes. */
export function releaseBundles(target) {
  return targetPlatform(target).bundles
}

/** Where `tauri build --target <triple>` leaves this target's bundles. */
export function bundleDirectory(target) {
  return `target/${target}/release/bundle`
}

/** What an update installs on this target: each artifact with its manifest key.
 *
 * The Linux suffixes are the plugin's own installer names (`Installer::name`
 * in `tauri-plugin-updater`), which it appends to `{os}-{arch}` for the bundle
 * type the running app was packaged as. */
export function updaterArtifacts(productName, version, target) {
  const bundle = bundleDirectory(target)
  const key = releaseTarget(target)
  if (targetPlatform(target).platform === "darwin")
    return [
      {
        key,
        built: `${bundle}/macos/${productName}.app.tar.gz`,
        published: `${productName}_${version}_${key}.app.tar.gz`,
      },
    ]
  // The bundler already names Linux packages per version and architecture, so
  // the .deb is published under the name it wrote.
  const { deb } = linuxBundles(productName, version, linuxBundleArchitecture(target))
  return [{ key: `${key}-deb`, built: `${bundle}/${deb}`, published: basename(deb) }]
}

/** The disk image's published name.
 *
 * The bundler already names disk images per architecture, so this is the name
 * it wrote — kept here so both published names come from one place. */
export function diskImageAssetName(productName, version, target) {
  return `${productName}_${version}_${bundleArchitecture(target, undefined)}.dmg`
}

/** Every file one target's build contributes, as built and as published.
 *
 * The signature is `createUpdaterArtifacts` writing `<artifact>.sig` beside the
 * artifact, signed with `TAURI_SIGNING_PRIVATE_KEY`. It is published too: the
 * manifest carries the same bytes, and having the file on the release is what
 * lets anyone check a download by hand. On Linux the update artifacts are also
 * what a person downloads; on macOS the disk image is. */
export function stagedAssets(productName, version, target) {
  const updates = updaterArtifacts(productName, version, target).flatMap(
    ({ built, published }) => [
      { built, published },
      { built: `${built}.sig`, published: `${published}.sig` },
    ],
  )
  if (targetPlatform(target).platform !== "darwin") return updates
  const image = diskImageAssetName(productName, version, target)
  return [
    ...updates,
    { built: `${bundleDirectory(target)}/dmg/${image}`, published: image },
  ]
}

/** The URL a release asset is downloaded from once the release is published. */
export function releaseAssetUrl(repository, tag, name) {
  return `https://github.com/${repository}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(name)}`
}

/** Base64, decoded only if that is genuinely what it is.
 *
 * `Buffer.from(text, "base64")` skips anything it does not recognise instead of
 * failing, so a sidecar with the right header and rubbish after it decodes
 * happily and keeps the rubbish. Re-encoding and comparing is what makes the
 * decode mean something. */
function strictBase64(encoded, source, what) {
  const bytes = Buffer.from(encoded, "base64")
  const canonical = bytes.toString("base64").replace(/=+$/, "")
  if (canonical !== encoded.replace(/=+$/, ""))
    throw new Error(`${source}: ${what} is not base64`)
  return bytes
}

/** Bytes of a minisign signature line: two for the algorithm, eight for the key
 * it was made with, sixty-four for the signature itself. Anything shorter is a
 * sidecar that was cut off. */
const SIGNATURE_BYTES = 74

/** Whether a `.sig` file holds what Tauri writes into one.
 *
 * The file is base64 of a minisign block: a comment line, the signature, a
 * trusted comment, and the signature over that. An empty or truncated one still
 * parses as a manifest field and fails only on a user's machine, at install, as
 * a rejected signature — so it is checked here, where it can still stop a
 * release.
 *
 * The block is parsed rather than sniffed. Testing only for the header let a
 * file containing nothing but `untrusted comment:` through, which is a sidecar
 * with no signature in it at all, and let trailing rubbish through with it.
 *
 * This says the signature is well formed, not that it verifies. What proves the
 * shipped public key can verify a signature from the release key is the pairing
 * gate (`src-tauri/tests/updater_key_pairing.rs`), which runs before any of
 * this; what it does not prove is that this artifact's bytes are what was
 * signed. That check needs the key here and is not done. */
export function signatureBlock(text, source) {
  const encoded = text.trim()
  if (!encoded) throw new Error(`${source} is empty: nothing signed this artifact`)

  const decoded = strictBase64(encoded, source, "the file").toString("utf8")
  const lines = decoded.split("\n")
  if (!lines[0]?.startsWith("untrusted comment:"))
    throw new Error(`${source} does not contain a minisign signature block`)

  const signature = lines[1]?.trim()
  if (!signature)
    throw new Error(
      `${source} has a comment and no signature after it: nothing signed this artifact`,
    )
  const bytes = strictBase64(signature, source, "the signature")
  if (bytes.length !== SIGNATURE_BYTES)
    throw new Error(
      `${source} carries ${bytes.length} bytes of signature, not ${SIGNATURE_BYTES}: it is truncated`,
    )

  // `Ed` is minisign's plain signature, `ED` the prehashed one Tauri writes.
  const algorithm = bytes.subarray(0, 2).toString("latin1")
  if (algorithm !== "Ed" && algorithm !== "ED")
    throw new Error(`${source} is not an ed25519 signature (algorithm ${algorithm})`)

  return encoded
}

/** The `platforms` map of a release, one entry per update artifact built.
 *
 * `targets` is what the release requires, not what happened to arrive. A
 * target whose build failed is absent from `signatures` and throws here, which
 * is the behaviour we want: half a release is not a release. */
export function releasePlatforms({
  productName,
  version,
  tag,
  repository,
  targets,
  signatures,
}) {
  return Object.fromEntries(
    targets.flatMap((target) =>
      updaterArtifacts(productName, version, target).map(({ key, published }) => {
        const signature = Object.hasOwn(signatures, key) ? signatures[key] : undefined
        if (!signature)
          throw new Error(
            `No signature for ${key}. Every platform and package format in a release ` +
              `must be in its manifest: one that is missing cannot update, and reports ` +
              `itself as already up to date.`,
          )
        return [key, { signature, url: releaseAssetUrl(repository, tag, published) }]
      }),
    ),
  )
}

/** The whole `latest.json` a release publishes. */
export function releaseUpdaterManifest({
  productName,
  version,
  tag,
  repository,
  targets,
  signatures,
  notes,
  published,
}) {
  return releaseManifest({
    version,
    notes,
    platforms: releasePlatforms({
      productName,
      version,
      tag,
      repository,
      targets,
      signatures,
    }),
    published,
  })
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main()

function main() {
  const root = resolve(import.meta.dirname, "../..")
  const args = process.argv.slice(2)
  const [command] = args
  const config = JSON.parse(
    readFileSync(resolve(root, "src-tauri/tauri.conf.json"), "utf8"),
  )
  if (command !== "stage" && command !== "manifest") {
    console.error(`Usage: release-assets.mjs stage|manifest [options]`)
    process.exit(2)
  }
  try {
    if (command === "stage") stage(root, args, config)
    else manifest(root, args, config)
  } catch (failure) {
    // The reasons a release stops here are written to be read by a person
    // deciding what to do next; a stack trace above them buries the sentence.
    console.error(failure.message)
    process.exit(1)
  }
}

/** Copy one target's build output under the names it is published with. */
function stage(root, args, config) {
  const target = option(args, "target", undefined)
  if (!target) throw new Error("--target is required")
  const into = resolve(root, option(args, "into", "release-staging"))
  mkdirSync(into, { recursive: true })
  for (const asset of stagedAssets(config.productName, config.version, target)) {
    if (!existsSync(resolve(root, asset.built)))
      throw new Error(
        `The build produced no ${asset.built}.\n` +
          `A missing .sig means the bundler had no TAURI_SIGNING_PRIVATE_KEY and signed\n` +
          `nothing; a missing archive, disk image or .deb means --bundles did\n` +
          `not ask for it.`,
      )
    copyFileSync(resolve(root, asset.built), resolve(into, asset.published))
    console.error(`  ${asset.built} -> ${asset.published}`)
  }
}

/** Read every target's staged signatures and write the release manifest. */
function manifest(root, args, config) {
  const directory = resolve(root, option(args, "directory", "release-assets"))
  const tag = option(args, "tag", process.env.GITHUB_REF_NAME)
  const repository = option(args, "repository", process.env.GITHUB_REPOSITORY)
  if (!tag) throw new Error("--tag is required")
  if (!repository) throw new Error("--repository is required")
  const targets = option(args, "targets", RELEASE_TARGETS.join(",")).split(",")
  // A target whose build failed leaves no signature here. Read it as absent
  // rather than as an error about a path, so the refusal that follows is the
  // one that explains what publishing without it would do.
  const signatures = Object.fromEntries(
    targets.flatMap((target) =>
      updaterArtifacts(config.productName, config.version, target).flatMap(
        ({ key, published }) => {
          const name = `${published}.sig`
          let text
          try {
            text = readFileSync(resolve(directory, name), "utf8")
          } catch {
            return []
          }
          // The signature is checked; so is the thing it signs being here at
          // all. A manifest naming a URL for an asset that was never uploaded
          // publishes cleanly and fails on a person's machine as a download
          // error — invisible from here afterwards, which is the reason this
          // step checks rather than trusting that staging already did.
          if (!existsSync(resolve(directory, published)))
            throw new Error(
              `${published} is not in the release assets, but ${name} is. ` +
                `Publishing this manifest would offer an update that cannot be downloaded.`,
            )
          return [[key, signatureBlock(text, name)]]
        },
      ),
    ),
  )
  const body = releaseUpdaterManifest({
    productName: config.productName,
    version: config.version,
    tag,
    repository,
    targets,
    signatures,
    // Empty unless a release says something. The default was the product name
    // and version, which the update tab already shows on its own line — it read
    // as a release whose notes were its own title. The panel has a written
    // sentence for the empty case; this is what makes that the state that ships.
    notes: option(args, "notes", ""),
    published: new Date().toISOString(),
  })
  const written = resolve(directory, "latest.json")
  writeFileSync(written, `${JSON.stringify(body, null, 2)}\n`)
  console.error(`  wrote ${written} for ${Object.keys(body.platforms).join(", ")}`)
}
