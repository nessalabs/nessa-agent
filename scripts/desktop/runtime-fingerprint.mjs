import { createHash } from "node:crypto"
import { lstatSync, readdirSync, readFileSync, readlinkSync, realpathSync } from "node:fs"
import { isAbsolute, join, relative, sep } from "node:path"

/** Hash the shipped tree, excluding only the generated root manifest. Names,
 * entry kinds, executable bits, and lengths frame the bytes unambiguously.
 * Absolute locations and timestamps do not identify a relocatable runtime.
 * The managed runtime this fingerprints is prepared and verified on macOS only,
 * so the executable bit it reads is a POSIX mode, not a platform-derived guess.
 */
export function runtimeFingerprint(directory) {
  const root = realpathSync(directory)
  const hash = createHash("sha256")
  function visit(path, name) {
    const stat = lstatSync(path)
    if (stat.isSymbolicLink()) {
      const resolved = relative(root, realpathSync(path))
      const target = readlinkSync(path)
      // Containment is decided by the resolved target: anything reaching outside
      // the bundle is rejected however it is spelled. An absolute target is
      // rejected as well, even when it resolves inside, because it names a
      // location the bundle cannot be moved away from. isAbsolute covers
      // Windows drive-absolute and UNC targets, matching the Rust fingerprint.
      if (resolved === ".." || resolved.startsWith(`..${sep}`) || isAbsolute(target))
        throw new Error(`Runtime link must stay inside the bundle: ${name}`)
      hash.update(JSON.stringify([name, "link", target]) + "\n")
    } else if (stat.isDirectory()) {
      hash.update(JSON.stringify([name, "directory"]) + "\n")
      for (const child of readdirSync(path).sort()) {
        if (name === "" && child === "manifest.json") continue
        visit(join(path, child), name ? `${name}/${child}` : child)
      }
    } else if (stat.isFile()) {
      const bytes = readFileSync(path)
      hash.update(JSON.stringify([name, "file", stat.mode & 0o111, bytes.length]) + "\n")
      hash.update(bytes)
    } else {
      throw new Error(`Unsupported runtime entry: ${name}`)
    }
  }
  visit(root, "")
  return hash.digest("hex")
}

/** Verify that a prepared or packaged runtime still matches its manifest. */
export function verifyRuntimeFingerprint(directory) {
  const manifest = JSON.parse(readFileSync(join(directory, "manifest.json"), "utf8"))
  const fingerprint = runtimeFingerprint(directory)
  if (manifest.fingerprint !== fingerprint)
    throw new Error(`Runtime fingerprint does not match its manifest: ${fingerprint}`)
  return fingerprint
}
