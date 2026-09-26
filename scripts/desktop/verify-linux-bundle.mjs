/**
 * What a Linux build must hold before it is published.
 *
 * Each selected package carries the fingerprinted runtime where the app looks
 * for it — `usr/lib/<productName>/runtime`, the Linux `resource_dir` in
 * tauri-utils — and can load the libraries the app needs at run time: the
 * `.deb` declares them for the package manager, the AppImage carries the
 * tray's indicator library inside itself. Nothing is code-signed on Linux; the
 * updater's `.sig` files are checked when a release is staged.
 */
import { execFileSync } from "node:child_process"
import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs"
import { tmpdir } from "node:os"
import { basename, dirname, join, resolve } from "node:path"
import { pathToFileURL } from "node:url"
import { linuxBundleArchitecture, linuxBundles } from "./bundle-architecture.mjs"
import { verifyRuntimeFingerprint } from "./runtime-fingerprint.mjs"

/** What an installed `.deb` cannot run without, stated here rather than read
 * from the config under test: the WebKitGTK the app links (wry's
 * `webkit2gtk-4.1`), without which it cannot open a window, and the indicator
 * library the tray loads with `dlopen`, without which it cannot make a tray. */
export const REQUIRED_DEB_PACKAGES = ["libwebkit2gtk-4.1-0", "libayatana-appindicator3-1"]

/** The package names a `Depends` field requires, alternatives included.
 *
 * `a (>= 1) | b, c` requires `a` or `b`, and `c`. Each clause is kept as the
 * list of names that satisfy it, versions dropped: this check is about what
 * is declared, and the version is the bundler's business. */
export function dependencyClauses(field) {
  return field
    .split(",")
    .map((clause) =>
      clause
        .split("|")
        .map((alternative) => alternative.trim().split(/[\s(:]/, 1)[0])
        .filter(Boolean),
    )
    .filter((clause) => clause.length > 0)
}

/** Each required package no clause of `field` names. */
export function missingDependencies(field, required) {
  const declared = new Set(dependencyClauses(field).flat())
  return required.filter((name) => !declared.has(name))
}

/** Whether a directory tree holds the tray's indicator library.
 *
 * The tray loads `libayatana-appindicator3` (or the older `libappindicator3`)
 * with `dlopen`, so nothing that follows ELF dependencies finds it; the
 * AppImage has to carry it explicitly. */
export function carriesIndicatorLibrary(directory) {
  return readdirSync(directory, { recursive: true }).some((path) => {
    // By file name: a recursive listing separates with the host's own
    // separator, which is not `/` everywhere this is tested.
    if (!/^lib(ayatana-)?appindicator3\.so/.test(basename(path))) return false
    // Followed through links: a dangling one or an empty file loads nothing.
    try {
      const target = statSync(join(directory, path))
      return target.isFile() && target.size > 0
    } catch {
      return false
    }
  })
}

function dpkgDeb(args, options) {
  try {
    return execFileSync("dpkg-deb", args, options)
  } catch (error) {
    if (error.code === "ENOENT")
      throw new Error(
        "Verifying a .deb needs dpkg-deb, which this host does not have. " +
          "Build it on Debian or Ubuntu.",
        { cause: error },
      )
    throw error
  }
}

/** The one candidate this build wrote: modified no earlier than the second
 * the build started. The version is not predicted: a build can merge another
 * one through `--config` (the updater harness builds an older app that way),
 * and a package an earlier build left under another version is not this
 * build's, whatever it is named. Which names are candidates is
 * `packageBuiltIn`'s to decide. */
export function builtPackage(candidates, startedAt, pattern) {
  if (!Number.isFinite(startedAt))
    throw new Error(`The build's start is not a time: ${startedAt}`)
  const since = Math.floor(startedAt / 1000) * 1000
  const built = candidates.filter(({ modified }) => modified >= since)
  if (built.length !== 1)
    throw new Error(
      `Expected one ${pattern} written by this build, found ` +
        `${built.length === 0 ? "none" : built.map(({ name }) => name).join(", ")}`,
    )
  return built[0].name
}

/** The package this build wrote under `bundle`, for a `linuxBundles` pattern
 * whose version is `*`. Only names that could be the package are examined, so
 * an unrelated entry that vanishes mid-listing cannot fail the check. */
export function packageBuiltIn(bundle, pattern, startedAt) {
  const directory = resolve(bundle, dirname(pattern))
  const [prefix, suffix] = basename(pattern).split("*")
  const entries = readdirSync(directory)
    .filter((name) => name.startsWith(prefix) && name.endsWith(suffix))
    .map((name) => ({ name, modified: statSync(join(directory, name)).mtimeMs }))
  return join(directory, builtPackage(entries, startedAt, basename(pattern)))
}

/** The Linux bundles this script can check. */
const CHECKED_BUNDLES = ["deb", "appimage"]

/** Each of a build's `--bundles` no check here covers. A build is not
 * "verified" by a run that looked at none of what it made, so the build
 * command asks this before it starts, and refuses. */
export function uncheckedBundles(bundles) {
  return bundles.split(",").filter((bundle) => !CHECKED_BUNDLES.includes(bundle))
}

/** What the build that ran this made, and when it began.
 *
 * Only as part of a build: `pnpm app:build` says both, and which package is
 * this build's is decided by when it was written (`builtPackage`), whatever
 * its version. A bundle no check here opens is refused, whoever asked. */
export function buildSelection(environment) {
  const selected = environment.NESSA_BUILD_BUNDLES
  const started = environment.NESSA_BUILD_STARTED
  if (!selected || !started)
    throw new Error("Run through `pnpm app:build`, which says what it built and when.")
  const unchecked = uncheckedBundles(selected)
  if (unchecked.length > 0)
    throw new Error(`No Linux check verifies the ${unchecked.join(", ")} bundle`)
  return { bundles: selected.split(","), started: Number(started) }
}

function withScratch(action) {
  const scratch = mkdtempSync(join(tmpdir(), "nessa-bundle-"))
  try {
    return action(scratch)
  } finally {
    rmSync(scratch, { recursive: true, force: true })
  }
}

function verifyDeb(path, { productName }) {
  const field = dpkgDeb(["--field", path, "Depends"], { encoding: "utf8" })
  const missing = missingDependencies(field, REQUIRED_DEB_PACKAGES)
  if (missing.length > 0)
    throw new Error(
      `${path} does not declare ${missing.join(", ")} (Depends: ${field.trim()})`,
    )
  withScratch((scratch) => {
    dpkgDeb(["--extract", path, scratch], { stdio: "inherit" })
    verifyRuntimeFingerprint(join(scratch, "usr/lib", productName, "runtime"))
  })
  console.error(`  verified ${path}`)
}

function verifyAppImage(path, { productName }) {
  withScratch((scratch) => {
    // `--appimage-extract` is the AppImage runtime's own: it needs no FUSE,
    // which a CI container rarely has, and it proves the file is one.
    // Its listing of every extracted file is noise; its errors are the reason.
    execFileSync(path, ["--appimage-extract"], {
      cwd: scratch,
      stdio: ["ignore", "ignore", "inherit"],
    })
    const root = join(scratch, "squashfs-root")
    verifyRuntimeFingerprint(join(root, "usr/lib", productName, "runtime"))
    if (!carriesIndicatorLibrary(root))
      throw new Error(`${path} does not carry the tray's appindicator library`)
  })
  console.error(`  verified ${path}`)
}

function main() {
  const { bundles, started } = buildSelection(process.env)
  const root = resolve(import.meta.dirname, "../..")
  const metadata = JSON.parse(
    execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
      cwd: root,
      encoding: "utf8",
    }),
  )
  const target = process.env.NESSA_BUILD_TARGET
  const bundle = target
    ? resolve(metadata.target_directory, target, "release/bundle")
    : resolve(metadata.target_directory, "release/bundle")
  const config = JSON.parse(readFileSync(resolve(root, "src-tauri/tauri.conf.json")))
  const patterns = linuxBundles(
    config.productName,
    "*",
    linuxBundleArchitecture(target, process.arch),
  )
  const product = { productName: config.productName }
  const verify = { deb: verifyDeb, appimage: verifyAppImage }
  for (const kind of bundles)
    verify[kind](packageBuiltIn(bundle, patterns[kind], started), product)
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main()
