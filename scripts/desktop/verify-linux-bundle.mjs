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
import { mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join, resolve } from "node:path"
import { pathToFileURL } from "node:url"
import {
  includesBundle,
  linuxBundleArchitecture,
  linuxBundles,
} from "./bundle-architecture.mjs"
import { verifyRuntimeFingerprint } from "./runtime-fingerprint.mjs"

/** The WebKitGTK the app is linked against (wry's `webkit2gtk-4.1`). The
 * bundler adds it to every `.deb`; it is checked because a package without
 * it installs and then cannot open a window. */
const WEBKIT_PACKAGE = "libwebkit2gtk-4.1-0"

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
  return readdirSync(directory, { recursive: true }).some((path) =>
    /(^|\/)lib(ayatana-)?appindicator3\.so/.test(path),
  )
}

function withScratch(action) {
  const scratch = mkdtempSync(join(tmpdir(), "nessa-bundle-"))
  try {
    return action(scratch)
  } finally {
    rmSync(scratch, { recursive: true, force: true })
  }
}

function verifyDeb(path, { productName, depends }) {
  const field = execFileSync("dpkg-deb", ["--field", path, "Depends"], {
    encoding: "utf8",
  })
  const missing = missingDependencies(field, [WEBKIT_PACKAGE, ...depends])
  if (missing.length > 0)
    throw new Error(
      `${path} does not declare ${missing.join(", ")} (Depends: ${field.trim()})`,
    )
  withScratch((scratch) => {
    execFileSync("dpkg-deb", ["--extract", path, scratch], { stdio: "inherit" })
    verifyRuntimeFingerprint(join(scratch, "usr/lib", productName, "runtime"))
  })
  console.error(`  verified ${path}`)
}

function verifyAppImage(path, { productName }) {
  withScratch((scratch) => {
    // `--appimage-extract` is the AppImage runtime's own: it needs no FUSE,
    // which a CI container rarely has, and it proves the file is one.
    execFileSync(path, ["--appimage-extract"], { cwd: scratch, stdio: "ignore" })
    const root = join(scratch, "squashfs-root")
    verifyRuntimeFingerprint(join(root, "usr/lib", productName, "runtime"))
    if (!carriesIndicatorLibrary(root))
      throw new Error(`${path} does not carry the tray's appindicator library`)
  })
  console.error(`  verified ${path}`)
}

function main() {
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
  const packages = linuxBundles(
    config.productName,
    config.version,
    linuxBundleArchitecture(target, process.arch),
  )
  const selected = process.env.NESSA_BUILD_BUNDLES
  const product = {
    productName: config.productName,
    depends: config.bundle.linux?.deb?.depends ?? [],
  }
  if (includesBundle(selected, config.bundle.targets, "deb"))
    verifyDeb(resolve(bundle, packages.deb), product)
  if (includesBundle(selected, config.bundle.targets, "appimage"))
    verifyAppImage(resolve(bundle, packages.appimage), product)
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main()
