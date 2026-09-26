export function bundleArchitecture(target, hostArchitecture) {
  if (target) {
    if (target.startsWith("x86_64")) return "x64"
    if (target === "universal-apple-darwin") return "universal"
    if (target.startsWith("aarch64")) return "aarch64"
    throw new Error(`Unsupported macOS target: ${target}`)
  }
  if (hostArchitecture === "x64") return "x64"
  if (hostArchitecture === "arm64") return "aarch64"
  throw new Error(`Unsupported macOS host architecture: ${hostArchitecture}`)
}

/** The architecture the Linux bundler writes into package names.
 *
 * Debian's spelling, for the `.deb` and the AppImage alike. x86_64 is the one
 * Linux target `prepare-linux.mjs` assembles a runtime for. */
export function linuxBundleArchitecture(target, hostArchitecture) {
  if (target === "x86_64-unknown-linux-gnu") return "amd64"
  if (!target && hostArchitecture === "x64") return "amd64"
  throw new Error(`Unsupported Linux target: ${target ?? hostArchitecture}`)
}

/** Where the Linux bundler leaves each package, relative to the bundle directory. */
export function linuxBundles(productName, version, architecture) {
  return {
    deb: `deb/${productName}_${version}_${architecture}.deb`,
    appimage: `appimage/${productName}_${version}_${architecture}.AppImage`,
  }
}

/** The bundles a build makes: those named with `--bundles`, else the config's. */
export function selectedBundles(selected, configured) {
  return selected
    ? selected.split(",")
    : configured === "all"
      ? ["all"]
      : Array.isArray(configured)
        ? configured
        : []
}

/** Whether the selected or configured bundle set includes `bundle`. */
export function includesBundle(selected, configured, bundle) {
  const targets = selectedBundles(selected, configured)
  return targets.includes("all") || targets.includes(bundle)
}
