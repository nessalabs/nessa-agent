#!/usr/bin/env node
/**
 * `pnpm ui:paths`: write `tsconfig.json`'s `paths` from the design system's
 * path table. What happens — written, left alone, or refused with a sentence
 * and a non-zero exit — is `writeTsconfigPaths`'s to decide
 * (`nessa-ui-paths.mjs`); this only hands it the real file.
 */
import { readFileSync, renameSync, rmSync, writeFileSync } from "node:fs"
import { fileURLToPath } from "node:url"

import { replaceWhole, writeTsconfigPaths } from "./nessa-ui-paths.mjs"

const file = fileURLToPath(new URL("../tsconfig.json", import.meta.url))
const result = writeTsconfigPaths(file, {
  read: (path) => readFileSync(path),
  write: (path, text) => replaceWhole(path, text, { writeFileSync, renameSync, rmSync }),
})
if ("refused" in result) {
  console.error(result.refused)
  process.exit(1)
}
