#!/usr/bin/env node
/** Credential-free verification of the retained provider contract. */
import { readFileSync } from "node:fs"
import { inspectCapture } from "./evidence.mjs"
const fixture = JSON.parse(
  readFileSync(new URL("./fixtures/codex-native.json", import.meta.url), "utf8"),
)
process.stdout.write(
  `${JSON.stringify({ source: fixture.source.kind, ...inspectCapture(fixture) })}\n`,
)
