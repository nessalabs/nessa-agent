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

/** Whether the selected or configured macOS bundle set includes a disk image. */
export function includesDiskImage(selected, configured) {
  const targets = selected
    ? selected.split(",")
    : configured === "all"
      ? ["all"]
      : Array.isArray(configured)
        ? configured
        : []
  return targets.includes("all") || targets.includes("dmg")
}
