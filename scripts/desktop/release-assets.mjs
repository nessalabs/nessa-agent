/**
 * What a release publishes, named once, and how two runners' builds become one
 * `latest.json`.
 *
 * Apple Silicon and Intel cannot be built together — `prepare-macos.mjs` refuses
 * a target triple that is not the host's and downloads Node for `process.arch`,
 * so a universal binary is not available to us. Two runners build separately and
 * produce identically named files: both bundlers write `Nessa.app.tar.gz`. A
 * GitHub release has one flat namespace, so uploading both as they are would
 * silently leave one architecture pointing at the other's bytes — which fails
 * far away, as a rejected signature on a user's machine.
 *
 *   macos-latest ─▶ aarch64 bundle ─┐  stage (rename)   ┌─▶ Nessa_0.1.0_darwin-aarch64.app.tar.gz
 *                                   ├──────────────────▶┤
 *   macos-13     ─▶ x86_64 bundle  ─┘                   └─▶ Nessa_0.1.0_darwin-x86_64.app.tar.gz
 *                                                              │
 *                                          manifest ◀──────────┘  (+ its .sig)
 *                                              │
 *                                        latest.json  ── one file, both keys
 *
 * `stage` runs on each build runner and renames that architecture's output to
 * the name it will be published under. `manifest` runs once afterwards, over
 * both staged sets, and writes the `latest.json` the updater endpoint serves.
 * It refuses to write one that is missing an architecture: a manifest with one
 * key is not a smaller release, it is a release the other half of the installed
 * base can never update from, and it fails silently as "no update available".
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
import { resolve } from "node:path"
import { pathToFileURL } from "node:url"
import { bundleArchitecture } from "./bundle-architecture.mjs"
import { option, releaseManifest, updaterTarget } from "./updater-manifest.mjs"

/** The architectures a release builds, in the order a manifest lists them. */
export const RELEASE_TARGETS = ["aarch64-apple-darwin", "x86_64-apple-darwin"]

/** The `{os}-{arch}` manifest key for a macOS target triple.
 *
 * Routed through the same `updaterTarget` the harness uses rather than spelled
 * out again, so there is one answer to what the plugin calls an architecture.
 * `universal-apple-darwin` is rejected rather than mapped: we cannot build it,
 * and a key for a bundle that does not exist is worse than an error here. */
export function releaseTarget(target) {
  const architecture = {
    aarch64: "arm64",
    x64: "x64",
  }[bundleArchitecture(target, undefined)]
  if (!architecture)
    throw new Error(`No single-architecture macOS release is built for ${target}`)
  return updaterTarget("darwin", architecture)
}

/** Where `tauri build --target <triple>` leaves this architecture's bundles. */
export function bundleDirectory(target) {
  return `target/${target}/release/bundle`
}

/** The update artifact's published name: one per architecture, never colliding. */
export function updaterAssetName(productName, version, target) {
  return `${productName}_${version}_${releaseTarget(target)}.app.tar.gz`
}

/** The disk image's published name.
 *
 * The bundler already names disk images per architecture, so this is the name
 * it wrote — kept here so both published names come from one place. */
export function diskImageAssetName(productName, version, target) {
  return `${productName}_${version}_${bundleArchitecture(target, undefined)}.dmg`
}

/** Every file one architecture's build contributes, as built and as published.
 *
 * The signature is `createUpdaterArtifacts` writing `<artifact>.sig` beside the
 * archive, signed with `TAURI_SIGNING_PRIVATE_KEY`. It is published too: the
 * manifest carries the same bytes, and having the file on the release is what
 * lets anyone check a download by hand. */
export function stagedAssets(productName, version, target) {
  const bundle = bundleDirectory(target)
  const updater = updaterAssetName(productName, version, target)
  const image = diskImageAssetName(productName, version, target)
  return [
    { built: `${bundle}/macos/${productName}.app.tar.gz`, published: updater },
    {
      built: `${bundle}/macos/${productName}.app.tar.gz.sig`,
      published: `${updater}.sig`,
    },
    { built: `${bundle}/dmg/${image}`, published: image },
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

/** The `platforms` map of a release, one entry per architecture built.
 *
 * `targets` is what the release requires, not what happened to arrive. An
 * architecture whose build failed is absent from `signatures` and throws here,
 * which is the behaviour we want: half a release is not a release. */
export function releasePlatforms({
  productName,
  version,
  tag,
  repository,
  targets,
  signatures,
}) {
  return Object.fromEntries(
    targets.map((target) => {
      const key = releaseTarget(target)
      const signature = signatures[key]
      if (!signature)
        throw new Error(
          `No signature for ${key}. Every architecture in a release must be in its ` +
            `manifest: one that is missing cannot update, and reports itself as ` +
            `already up to date.`,
        )
      const name = updaterAssetName(productName, version, target)
      return [key, { signature, url: releaseAssetUrl(repository, tag, name) }]
    }),
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

/** Copy one architecture's build output under the names it is published with. */
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
          `nothing; a missing archive or disk image means --bundles did not ask for it.`,
      )
    copyFileSync(resolve(root, asset.built), resolve(into, asset.published))
    console.error(`  ${asset.built} -> ${asset.published}`)
  }
}

/** Read both architectures' staged signatures and write the release manifest. */
function manifest(root, args, config) {
  const directory = resolve(root, option(args, "directory", "release-assets"))
  const tag = option(args, "tag", process.env.GITHUB_REF_NAME)
  const repository = option(args, "repository", process.env.GITHUB_REPOSITORY)
  if (!tag) throw new Error("--tag is required")
  if (!repository) throw new Error("--repository is required")
  const targets = option(args, "targets", RELEASE_TARGETS.join(",")).split(",")
  // An architecture whose build failed leaves no signature here. Read it as
  // absent rather than as an error about a path, so the refusal that follows is
  // the one that explains what publishing without it would do.
  const signatures = Object.fromEntries(
    targets.flatMap((target) => {
      const name = `${updaterAssetName(config.productName, config.version, target)}.sig`
      let text
      try {
        text = readFileSync(resolve(directory, name), "utf8")
      } catch {
        return []
      }
      return [[releaseTarget(target), signatureBlock(text, name)]]
    }),
  )
  const body = releaseUpdaterManifest({
    productName: config.productName,
    version: config.version,
    tag,
    repository,
    targets,
    signatures,
    notes: option(args, "notes", `${config.productName} ${config.version}`),
    published: new Date().toISOString(),
  })
  const written = resolve(directory, "latest.json")
  writeFileSync(written, `${JSON.stringify(body, null, 2)}\n`)
  console.error(`  wrote ${written} for ${Object.keys(body.platforms).join(", ")}`)
}
