#!/usr/bin/env node
import { launch, parseArgs, USAGE } from "./run.mjs"

const parsed = parseArgs(process.argv.slice(2))
if (parsed.mode === "help") {
  process.stderr.write(USAGE)
  process.exit(1)
}
if (parsed.mode === "error") {
  console.error(parsed.error)
  process.exit(1)
}

try {
  process.exit(launch(parsed.mode) ?? 1)
} catch (error) {
  console.error(error instanceof Error ? error.message : error)
  process.exit(1)
}
