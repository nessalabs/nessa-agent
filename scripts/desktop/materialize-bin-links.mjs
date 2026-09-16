import {
  chmodSync,
  copyFileSync,
  lstatSync,
  readdirSync,
  realpathSync,
  renameSync,
  statSync,
} from "node:fs"
import { join } from "node:path"

// Tauri copies npm's .bin links as files. Put the prepared runtime in that same
// final shape before hashing it so signing and bundling cannot change identity.
export function materializeBinLinks(nodeModules) {
  const bin = join(nodeModules, ".bin")
  for (const name of readdirSync(bin)) {
    const link = join(bin, name)
    if (!lstatSync(link).isSymbolicLink()) continue
    const target = realpathSync(link)
    const replacement = `${link}.nessa-materialized`
    copyFileSync(target, replacement)
    chmodSync(replacement, statSync(target).mode)
    renameSync(replacement, link)
  }
}
