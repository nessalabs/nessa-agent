#!/usr/bin/env node
/**
 * Contract checks for docs/codebase-structure.md and docs/ARCHITECTURE.md.
 * Failures are the rule plus the file that broke it — not a style opinion.
 */
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs"
import { execFileSync } from "node:child_process"
import { dirname, join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import {
  hasImmediateCfg,
  hasNoImmediateCfg,
} from "./architecture/platform-boundaries.mjs"
import { overlayPlacementViolations } from "./architecture/overlay-placement.mjs"
import {
  normalizedPath,
  rustBoundaryViolations,
  workspaceRustSourceRoots,
} from "./architecture/rust-boundaries.mjs"

const root = join(dirname(fileURLToPath(import.meta.url)), "..")
const src = join(root, "src")
const failures = []

function walk(dir) {
  const files = []
  for (const name of readdirSync(dir)) {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) files.push(...walk(path))
    else if (/\.(ts|tsx|js|mjs)$/.test(name)) files.push(path)
  }
  return files
}

function rel(path) {
  return normalizedPath(relative(root, path))
}

function fail(path, rule) {
  failures.push(`${rel(path)}: ${rule}`)
}

function rustFiles(directory) {
  const files = []
  for (const name of readdirSync(directory)) {
    const path = join(directory, name)
    if (statSync(path).isDirectory()) files.push(...rustFiles(path))
    else if (name.endsWith(".rs")) files.push(path)
  }
  return files
}

const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    cwd: root,
    encoding: "utf8",
  }),
)
for (const rustRoot of workspaceRustSourceRoots(root, metadata)) {
  const source = join(rustRoot, "src")
  if (!existsSync(source)) continue
  for (const file of rustFiles(source)) {
    const text = readFileSync(file, "utf8")
    for (const violation of rustBoundaryViolations(rel(file), text)) {
      fail(file, violation)
    }
    for (const violation of overlayPlacementViolations(rel(file), text)) {
      fail(file, violation)
    }
  }
}

if (existsSync(join(src, "utils.ts")) || existsSync(join(src, "utils.tsx"))) {
  fail(join(src, "utils.ts"), "there is no utils module")
}

const srcRootAllowed = new Set(["main.tsx", "store.ts", "icon-preview.tsx"])
for (const name of readdirSync(src)) {
  const path = join(src, name)
  if (statSync(path).isFile() && /\.(ts|tsx)$/.test(name) && !srcRootAllowed.has(name)) {
    fail(path, "src root is the composition root; feature code belongs in a vertical")
  }
}

function importedPaths(text) {
  return [...text.matchAll(/from\s+["']([^"']+)["']/g)].map((match) => match[1])
}

for (const file of walk(src)) {
  const text = readFileSync(file, "utf8")
  const path = rel(file)
  const imports = importedPaths(text)
  const inComposition = path.startsWith("src/composition/")
  // A vertical's `testing.ts` is how another vertical's tests reach what they
  // need without importing internals. Product code has no business there: it
  // would be a second, unchecked public surface.
  if (!path.endsWith(".test.ts") && !path.endsWith(".test.tsx")) {
    if (imports.some((item) => /(?:^|\/)testing$/.test(item)))
      fail(file, "only tests import a vertical's testing entry")
  }
  // Test fixtures must not introduce model imports across the public boundary.
  if (!path.startsWith("src/conversation/") && path.endsWith(".test.ts")) {
    if (imports.some((item) => /(?:^|\/)conversation\/model(?:\/|$)/.test(item))) {
      fail(
        file,
        "external tests use public contracts or literal fixtures, not conversation model internals",
      )
    }
  }

  if (
    !inComposition &&
    path !== "src/main.tsx" &&
    path !== "src/store.ts" &&
    !path.endsWith(".test.ts")
  ) {
    if (imports.some((item) => /(?:^|\/)composition(?:\/|$)/.test(item))) {
      fail(file, "features must not import the composition root")
    }
  }

  // Every vertical, not a list of them. These rules used to name `conversation`
  // and `session`, so a feature that grew a `model/` or `application/` later —
  // `panel/` did — got no rules at all and its first React import passed. The
  // layer a file is in is what decides what it may import, whichever feature it
  // belongs to.
  const vertical = /^src\/([^/]+)\/(model|application)\//.exec(path)
  const feature = vertical?.[1]
  const layer = vertical?.[2]
  const inLayerRules = Boolean(vertical) && !path.endsWith(".test.ts")

  if (inLayerRules && layer === "model") {
    if (
      imports.some((item) => /(?:^|\/)(?:application|adapters|ui)(?:\/|$)/.test(item))
    ) {
      fail(file, `${feature} model imports nothing outward`)
    }
  }

  if (inLayerRules && layer === "application") {
    if (imports.some((item) => /(?:^|\/)adapters(?:\/|$)/.test(item))) {
      fail(file, `${feature} use cases import the model and ports, not adapters`)
    }
    if (imports.some((item) => /(?:^|\/)ui(?:\/|$)/.test(item))) {
      fail(file, `${feature} use cases must not import the UI`)
    }
  }

  if (inLayerRules) {
    if (/from\s+["']react["']/.test(text) || /from\s+["']react\//.test(text)) {
      fail(file, `${feature} model/use cases must not import React`)
    }
    if (/@tauri-apps/.test(text)) {
      fail(file, `${feature} model/use cases must not import the host`)
    }
    if (/redux/i.test(text)) {
      fail(file, `${feature} model/use cases must not import the store`)
    }
  }

  if (
    path.startsWith("src/conversation/") &&
    /from\s+["'][^"']*\/host(?:\/window)?["']/.test(text)
  ) {
    fail(file, "the conversation vertical does not talk to the host; the panel does")
  }

  if (path.startsWith("src/conversation/ui/")) {
    if (/application\/internal/.test(text) || /application\/usecases/.test(text)) {
      fail(file, "the UI reads the projection; it does not import gateway internals")
    }
    if (/Linux[A-Z]/.test(text) || /data-host=/.test(text)) {
      fail(file, "host policy belongs in src/host, as a HostFeatures field")
    }
  }

  if (!path.startsWith("src/conversation/") && !path.endsWith(".test.ts")) {
    for (const item of imports) {
      if (!/conversation/.test(item)) continue
      const barrel =
        /(?:^|\/)conversation$/.test(item) || /(?:^|\/)conversation\/index$/.test(item)
      const slice = /conversation\/adapters\/store\/slice$/.test(item)
      const identity = /conversation\/model$/.test(item)
      if (inComposition && /\/(?:adapters\/|application\/ports$)/.test(item)) continue
      if (path === "src/store.ts" && slice) continue
      if (path === "src/icon-preview.tsx" && identity) continue
      if (barrel) continue
      fail(file, "other modules import the conversation barrel, not its internals")
    }
  }

  if (!path.startsWith("src/session/") && !path.endsWith(".test.ts")) {
    for (const item of imports) {
      if (!/(?:^|\/)session(?:\/|$)/.test(item) && !item.endsWith("/session")) continue
      // Avoid matching "session" inside unrelated paths; require session segment.
      if (!/(?:^|[./])session(?:\/|$)/.test(item)) continue
      const barrel = /(?:^|\/)session$/.test(item) || /(?:^|\/)session\/index$/.test(item)
      const slice = /session\/adapters\/store\/slice$/.test(item)
      if (inComposition && /\/(?:adapters\/|application\/ports$)/.test(item)) continue
      if (path === "src/store.ts" && slice) continue
      if (barrel) continue
      fail(file, "other modules import the session barrel, not its internals")
    }
  }

  if (path.startsWith("src/session/ui/")) {
    if (/adapters\/client/.test(text) || /adapters\/lifecycle/.test(text)) {
      fail(file, "session UI reads the projection; it does not open the socket")
    }
  }

  if (
    path.startsWith("src/session/model/") &&
    !path.endsWith(".test.ts") &&
    imports.some((item) => /(?:^|\/)(?:adapters|ui)(?:\/|$)/.test(item))
  ) {
    fail(file, "session model imports nothing outward")
  }

  if (
    /from\s+["']@tauri-apps(?:\/[^"']*)?["']/.test(text) ||
    /import\(\s*["']@tauri-apps/.test(text)
  ) {
    if (path !== "src/host/window.ts") {
      fail(file, "only src/host/window.ts may import @tauri-apps")
    }
  }

  if (!path.startsWith("src/host/") && /hostKind\s*===/.test(text)) {
    fail(file, "hostKind === belongs in src/host, as a HostFeatures field")
  }

  if (
    (path === "src/app.tsx" || path.endsWith("/app.tsx")) &&
    /\buseEffect\b/.test(text)
  ) {
    fail(file, "app.tsx renders; effects belong in a hook")
  }

  // The setup window is created hidden and revealed by its own page. A hidden
  // macOS window is never drawn, so that page is served no animation frames:
  // anything the reveal waits on a frame for, it waits on for good. Setup then
  // runs — sound and all — behind a window nobody ever sees.
  const revealsTheSetupWindow =
    path === "src/onboarding/ui/setup-gate.tsx" ||
    path === "src/onboarding/ui/reveal-on-first-render.ts" ||
    path === "src/host/window.ts"
  if (revealsTheSetupWindow && /requestAnimationFrame\s*\(/.test(text)) {
    fail(
      file,
      "the setup window is hidden until its page reveals it, and a hidden window is served no animation frames; see src/onboarding/ui/reveal-on-first-render.ts",
    )
  }

  if (path === "src/store.ts" && /from\s+["']\.\/app["']/.test(text)) {
    fail(file, "the store must not import the panel chrome")
  }

  if ((path === "src/app.tsx" || path.endsWith("/app.tsx")) && /Linux[A-Z]/.test(text)) {
    fail(
      file,
      "host policy belongs in src/host; do not name Linux components in the chrome",
    )
  }
}

const macosRetirementBoundaries = [
  {
    path: "crates/nessa-server/src/composition/root.rs",
    declaration: "use crate::desktop_runtime::{",
  },
  {
    path: "crates/nessa-server/src/desktop_runtime/application/mod.rs",
    declaration: "mod retirement;",
  },
  {
    path: "crates/nessa-server/src/desktop_runtime/infrastructure/mod.rs",
    declaration: "mod files;",
  },
  {
    path: "crates/nessa-server/src/desktop_runtime/domain/mod.rs",
    declaration: "pub(crate) use retirement::{",
  },
  {
    path: "crates/nessa-server/src/desktop_runtime/domain/retirement.rs",
    declaration: "pub(crate) struct RetirementRequest",
  },
  {
    path: "crates/nessa-server/src/desktop_runtime/domain/retirement.rs",
    declaration: "pub(crate) struct RetirementFence",
  },
  {
    path: "crates/nessa-server/src/desktop_runtime/domain/retirement.rs",
    declaration: "pub(crate) struct RetirementCause",
  },
  {
    path: "crates/nessa-server/src/desktop_runtime/domain/retirement.rs",
    declaration: "pub(crate) fn validate_retirement_evidence",
  },
  {
    path: "crates/nessa-server/src/conversation/application/service.rs",
    declaration: "pub(crate) fn retirement_cause",
  },
  {
    path: "crates/nessa-server/src/composition/root.rs",
    declaration: "let retirement_clock",
  },
  {
    path: "crates/nessa-server/src/composition/root.rs",
    declaration: "let retirement_files",
  },
]

for (const boundary of macosRetirementBoundaries) {
  const file = join(root, boundary.path)
  const text = readFileSync(file, "utf8")
  if (!hasImmediateCfg(text, boundary.declaration)) {
    fail(
      file,
      "managed gateway retirement is a macOS production capability; gate its module boundary so other targets do not compile unused lifecycle code",
    )
  }
}

const portableRuntimeBoundaries = [
  {
    path: "crates/nessa-server/src/desktop_runtime/domain/mod.rs",
    declaration: "pub(crate) use retirement::RunningRuntime;",
  },
  {
    path: "crates/nessa-server/src/desktop_runtime/domain/retirement.rs",
    declaration: "pub(crate) struct RunningRuntime",
  },
]

for (const boundary of portableRuntimeBoundaries) {
  const file = join(root, boundary.path)
  const text = readFileSync(file, "utf8")
  if (!hasNoImmediateCfg(text, boundary.declaration)) {
    fail(
      file,
      "runtime incarnation identity is portable health evidence; do not hide it behind a target cfg",
    )
  }
}

if (failures.length > 0) {
  console.error("architecture check failed:\n")
  for (const line of failures) console.error(`  ${line}`)
  process.exit(1)
}

console.log("architecture check passed")
