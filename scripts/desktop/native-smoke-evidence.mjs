import { mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"

/** Retain bounded, non-secret diagnostics after the native smoke cleanup finishes. */
export function retainNativeSmokeFailure(root, instance, { logs, metadata }) {
  const directory = join(root, instance)
  mkdirSync(directory, { recursive: true, mode: 0o700 })
  writeFileSync(join(directory, "harness.log"), logs, { mode: 0o600 })
  writeFileSync(
    join(directory, "metadata.json"),
    `${JSON.stringify(metadata, null, 2)}\n`,
    { mode: 0o600 },
  )
  return directory
}
