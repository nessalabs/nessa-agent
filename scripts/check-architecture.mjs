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
import { setupGatePlacementViolations } from "./architecture/setup-gate-placement.mjs"
import { standDownPlacementViolations } from "./architecture/stand-down-placement.mjs"
import { composerBudgetViolations } from "./architecture/composer-budget.mjs"
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

// Entry points, and the dev-only previews that are entry points too: each one
// composes a vertical from outside rather than adding feature code to the root.
const srcRootAllowed = new Set([
  "main.tsx",
  "store.ts",
  "icon-preview.tsx",
  "transcript-preview.tsx",
  "messages-preview.tsx",
])
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
  for (const violation of setupGatePlacementViolations(path, text)) {
    fail(file, violation)
  }
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
    // A client SDK is outward too: a model states product rules in its own
    // terms, and the adapter that talks to the gateway is where they meet the
    // wire's. Written as a refusal with named exceptions rather than a list of
    // features to check, so a vertical that grows a model later is held to this
    // from its first line instead of from whenever somebody adds it here.
    //
    // `session` is design: that model *is* the wire session, and describing it
    // in other words would be describing something else. `onboarding` is not —
    // `model/shortcut-display.ts` reads the generated `ShortcutsDocument` to
    // find the summon accelerator, which is the same leak this rule exists to
    // stop. It is named here so it stays visible, and so that removing it is a
    // change to that vertical rather than a precondition for this one.
    const modelMayReadTheWire = feature === "session" || feature === "onboarding"
    if (
      !modelMayReadTheWire &&
      imports.some(
        (item) => item === "@nessa/client" || item.startsWith("@nessa/client/"),
      )
    )
      fail(file, `${feature} model does not import the client SDK`)
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

  // Whether this surface can say where a file is decides two things that have
  // to agree: the notice over the composer, and the sentence a refused send
  // gets. They disagreed, because the panel asked the host and composition
  // asked it again — one fact with two readers is a seam that generates the
  // divergence it was meant to prevent. The conversation vertical is already
  // barred from the host above; the panel is the other reader, and it takes
  // the answer as `canChoosePaths` instead of asking.
  if (
    path.startsWith("src/panel/") &&
    !path.endsWith(".test.ts") &&
    /hasNativeHost\s*\(/.test(text)
  ) {
    fail(
      file,
      "the panel takes canChoosePaths from composition; it does not ask the host again",
    )
  }

  if ((path === "src/app.tsx" || path.endsWith("/app.tsx")) && /Linux[A-Z]/.test(text)) {
    fail(
      file,
      "host policy belongs in src/host; do not name Linux components in the chrome",
    )
  }
}

// The ceiling over the composer's notices is a stylesheet rule, so nothing
// else in this file would notice it being deleted.
const stylesheet = join(src, "styles.css")
for (const violation of composerBudgetViolations(readFileSync(stylesheet, "utf8"))) {
  fail(stylesheet, violation)
}

// This gate runs on the Rust jobs, where there is no `pnpm install` and so no
// `node_modules` at all — `.github/workflows/local-auth.yml` runs it and its
// tests with bare Node before the gateway harness is even built. So everything
// it reaches may import Node and its own neighbours and nothing else. That was
// the design and nothing said so: a rule here grew a `typescript` import, every
// local run passed, and four CI jobs died on the first line of the script. A
// check that needs a parser belongs in `scripts/eslint/`, which runs where the
// dependencies are.
const architecture = join(root, "scripts", "architecture")
for (const file of [
  join(root, "scripts", "check-architecture.mjs"),
  join(root, "scripts", "check-runtime-dependencies.mjs"),
  ...walk(architecture),
]) {
  const text = readFileSync(file, "utf8")
  // A side-effect import and a dynamic one reach a package just as surely as a
  // named one, so all three are read. Anchored to the start of a line, which is
  // also what keeps a comment about imports from reading as one.
  const specifiers = [
    ...importedPaths(text),
    ...[...text.matchAll(/^\s*import\s*\(?\s*["']([^"']+)["']/gm)].map(
      (match) => match[1],
    ),
  ]
  for (const specifier of specifiers) {
    if (specifier.startsWith("node:") || specifier.startsWith(".")) continue
    fail(
      file,
      `the architecture check imports "${specifier}"; it runs on the Rust jobs with bare Node and no node_modules, so it may import node: builtins and its own neighbours and nothing else`,
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

// A script rather than product source, so it is read by path rather than by
// the walk over `src/`.
{
  const file = join(root, "scripts/dev-agent-config.mjs")
  const text = readFileSync(file, "utf8")
  for (const violation of standDownPlacementViolations(rel(file), text)) {
    fail(file, violation)
  }
}

if (failures.length > 0) {
  console.error("architecture check failed:\n")
  for (const line of failures) console.error(`  ${line}`)
  process.exit(1)
}

console.log("architecture check passed")
