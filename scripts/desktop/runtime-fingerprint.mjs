import { createHash } from "node:crypto"
import { lstatSync, readdirSync, readFileSync, readlinkSync, realpathSync } from "node:fs"
import { isAbsolute, join, relative, sep } from "node:path"

/** Hash the shipped tree, excluding only the generated root manifest. Names,
 * entry kinds, executable bits, and lengths frame the bytes unambiguously.
 * Absolute locations and timestamps do not identify a relocatable runtime.
 */
export function runtimeFingerprint(directory) {
  const root = realpathSync(directory)
  const hash = createHash("sha256")
  function visit(path, name) {
    const stat = lstatSync(path)
    if (stat.isSymbolicLink()) {
      const resolved = relative(root, realpathSync(path))
      const target = readlinkSync(path)
      // A link that names a location rather than a bundle-relative path does not
      // survive relocation, including a Windows drive-absolute or UNC target.
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
