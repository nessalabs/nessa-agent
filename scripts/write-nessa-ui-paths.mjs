#!/usr/bin/env node
/**
 * `pnpm ui:paths`: write `tsconfig.json`'s `paths` from the design system's
 * path table (`nessa-ui-paths.mjs`), keeping the rest of the file. The file is
 * written only when that changes it, so running this on a current file
 * touches nothing. A file that cannot be read, is not a plain JSON object, or
 * cannot be written is refused with a sentence and a non-zero exit.
 */
import { readFileSync, writeFileSync } from "node:fs"
import { fileURLToPath } from "node:url"

import { withTsconfigPaths } from "./nessa-ui-paths.mjs"

const file = fileURLToPath(new URL("../tsconfig.json", import.meta.url))
try {
  const before = readFileSync(file, "utf8")
  const after = withTsconfigPaths(before)
  if (after !== before) writeFileSync(file, after)
} catch (error) {
  console.error(error.message)
  process.exit(1)
}
