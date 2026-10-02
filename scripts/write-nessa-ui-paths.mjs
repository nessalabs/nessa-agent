#!/usr/bin/env node
/**
 * `pnpm ui:paths`: write `tsconfig.json`'s `paths` from the design system's
 * path table (`nessa-ui-paths.mjs`), keeping the rest of the file and its line
 * endings. A file that is not plain JSON is refused with a sentence and a
 * non-zero exit, and left as it was.
 */
import { readFileSync, writeFileSync } from "node:fs"
import { fileURLToPath } from "node:url"

import { withTsconfigPaths } from "./nessa-ui-paths.mjs"

const file = fileURLToPath(new URL("../tsconfig.json", import.meta.url))
let written
try {
  written = withTsconfigPaths(readFileSync(file, "utf8"))
} catch (error) {
  console.error(error.message)
  process.exit(1)
}
writeFileSync(file, written)
