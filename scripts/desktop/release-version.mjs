/**
 * Whether the version a release tag claims is the version the build will carry.
 *
 * Three files name the version independently — `package.json`,
 * `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json` — and only the last of
 * them reaches the updater. A tag that disagrees with `tauri.conf.json`
 * publishes `latest.json` under one version describing a bundle that reports
 * another, so every installed app either sees an update it already has or never
 * sees one at all. Nothing later in the release fails; it just quietly does not
 * work. So this runs first, before a runner is paid for, and stops the release.
 *
 *   tag v0.1.0 ─┐
 *               ├─▶ agreement ─▶ Agreed(version) ─▶ everything downstream
 *   package.json│                     │
 *   Cargo.toml  │                     └─▶ Disagreed(sources) ─▶ release stops
 *   tauri.conf.json
 *
 * The parsing and the rule are pure and tested; the CLI at the bottom is the
 * only part that reads files. On success it prints the agreed version, and
 * nothing else, to stdout — the workflow captures that as the version every
 * later job names artifacts with, so no job re-derives it.
 *
 *   node scripts/desktop/release-version.mjs --tag v0.1.0
 */

import { readFileSync } from "node:fs"
import { resolve } from "node:path"
import { pathToFileURL } from "node:url"
import { option } from "./updater-manifest.mjs"

/** The version in `package.json`, or `undefined` if it does not declare one. */
export function packageVersion(text) {
  const version = JSON.parse(text).version
  return typeof version === "string" ? version : undefined
}

/** The version in `tauri.conf.json`: the one the running app reports. */
export function tauriConfigVersion(text) {
  const version = JSON.parse(text).version
  return typeof version === "string" ? version : undefined
}

/** The literal version in a Cargo manifest's own `[package]` table.
 *
 * Deliberately not a TOML parser and deliberately not clever: a workspace
 * inheritance (`version.workspace = true`) or a missing key reads as absent,
 * which fails the gate. A release must not proceed on a version this could not
 * actually read, and a false pass here is the expensive direction. */
export function cargoVersion(text) {
  let inPackage = false
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim()
    if (trimmed.startsWith("[")) {
      inPackage = trimmed === "[package]"
      continue
    }
    if (!inPackage) continue
    const declared = trimmed.match(/^version\s*=\s*"([^"]*)"\s*(?:#.*)?$/)
    if (declared) return declared[1]
  }
  return undefined
}

/** The version a release tag names, or `undefined` if it names none.
 *
 * `v` then three dot-separated numbers, with an optional pre-release suffix,
 * because that is what the tag has to be for the version comparison the updater
 * plugin performs to mean anything. A tag this does not recognize is not a
 * release tag, and guessing at one would be worse than refusing it. */
export function taggedVersion(tag) {
  const named = tag?.trim().replace(/^refs\/tags\//, "")
  const matched = named?.match(/^v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)$/)
  return matched ? matched[1] : undefined
}

/**
 * Whether a version names a pre-release — `0.2.0-rc.1` does, `0.2.0` does not.
 *
 * GitHub keeps this apart from the tag's spelling: a release is a pre-release
 * because it is flagged one, not because its name has a suffix. A draft created
 * without the flag becomes an ordinary release when published, which puts a
 * release candidate in front of every installed app — the updater reads the
 * newest non-prerelease, and an unflagged rc is exactly that.
 */
export function isPrerelease(version) {
  return /-/.test(version ?? "")
}

/** Whether the tag and every file that names a version say the same thing.
 *
 * Typed rather than boolean: `Disagreed` carries what each source actually
 * said, because the only useful thing to read after this fails is the list of
 * which file is behind. */
export function versionAgreement({ tag, sources }) {
  const tagged = taggedVersion(tag)
  if (!tagged)
    return {
      kind: "Unnamed",
      tag,
      reason:
        `${tag ?? "(no tag)"} is not a release tag. A release tag is v followed by a ` +
        `semantic version, such as v0.1.0.`,
    }
  const disagreeing = sources.filter((source) => source.version !== tagged)
  if (disagreeing.length > 0) return { kind: "Disagreed", tagged, sources, disagreeing }
  return { kind: "Agreed", version: tagged }
}

/** The agreement failure, written out the way a person has to act on it. */
export function agreementReport(agreement) {
  if (agreement.kind === "Unnamed") return agreement.reason
  const said = (source) =>
    `  ${source.name}: ${source.version ?? "(no version declared)"}`
  return [
    `The tag ${agreement.tagged} does not agree with the version this build would carry.`,
    ...agreement.sources.map(said),
    ``,
    `The updater matches on the version in src-tauri/tauri.conf.json, so a release`,
    `published under a different number describes a bundle that reports another one:`,
    `installed apps then see no update, or the same update forever, with no error.`,
    ``,
    `Set every file above to ${agreement.tagged}, or tag the version they already`,
    `declare. Nothing was built and nothing was published.`,
  ].join("\n")
}

/** The three files that name a version, read from a repository checkout. */
export function declaredVersions(root, read = readFileSync) {
  return [
    {
      name: "package.json",
      version: packageVersion(read(resolve(root, "package.json"), "utf8")),
    },
    {
      name: "src-tauri/Cargo.toml",
      version: cargoVersion(read(resolve(root, "src-tauri/Cargo.toml"), "utf8")),
    },
    {
      name: "src-tauri/tauri.conf.json",
      version: tauriConfigVersion(
        read(resolve(root, "src-tauri/tauri.conf.json"), "utf8"),
      ),
    },
  ]
}

// Imported by its tests, run by the workflow. Only the second does any I/O.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main()

function main() {
  const root = resolve(import.meta.dirname, "../..")
  const tag = option(process.argv.slice(2), "tag", process.env.GITHUB_REF_NAME)
  const agreement = versionAgreement({ tag, sources: declaredVersions(root) })
  if (agreement.kind !== "Agreed") {
    console.error(agreementReport(agreement))
    process.exit(1)
  }
  // Data, not a diagnostic: the workflow reads one of these per call. The
  // channel is asked for here rather than derived from the version in shell,
  // so what counts as a pre-release is decided in one place and tested.
  const wanted = option(process.argv.slice(2), "print", "version")
  if (wanted === "version") process.stdout.write(`${agreement.version}\n`)
  else if (wanted === "prerelease")
    process.stdout.write(`${isPrerelease(agreement.version)}\n`)
  else {
    console.error(`--print takes "version" or "prerelease", not ${wanted}`)
    process.exit(1)
  }
}
