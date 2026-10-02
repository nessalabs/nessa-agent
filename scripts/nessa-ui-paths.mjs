/**
 * Where an import of the design system lands in its source: the one table.
 *
 * Nessa UI is consumed as source (see `vite.config.ts` for why), so three
 * tools each resolve its import paths: TypeScript (`tsconfig.json`'s
 * `paths`), Vite, and Vitest. They were three hand-kept copies, and they had
 * already parted: Vitest sent `@nessa-ui/react/app-shell` to a components
 * directory that does not exist. Now Vite and Vitest build their aliases from
 * this table. `tsconfig.json` cannot import code, so its `paths` are written
 * from it (`pnpm ui:paths`), and `scripts/check-architecture.mjs` fails when
 * they are not exactly what that writes (`tsconfigPathViolations`).
 *
 * Each rule maps an import prefix to a directory of the package's `src/`:
 *
 * - `@nessa-ui/react/app-shell`, the one composite: a whole specifier, matched
 *   exactly by all three tools, so none of them reads `app-shell/…` into it.
 * - `@nessa-ui/react/lib/<name>`: the registry libraries, such as the shared
 *   size observer, which the package's entry does not export.
 * - `@nessa-ui/react/<component>`: everything else under the namespace.
 * - `@/components/`, `@/lib/`, `@/provider/`: the package's own internal
 *   alias, scoped to the three prefixes its source uses rather than a bare
 *   `@`, which would also capture any `@/…` this app later writes for itself.
 *
 * Order is not part of the table: the derivers put a whole specifier and a
 * longer prefix ahead of a shorter one, so `lib/` is never claimed by the
 * components' rule. The architecture check imports this on bare Node with no
 * `node_modules`, so it may import only Node's builtins and its neighbours —
 * held by that check's own import rule, which reads this file. Its types are
 * `nessa-ui-paths.d.mts`'s.
 */

import { readFileSync, writeFileSync } from "node:fs"
import { fileURLToPath, pathToFileURL } from "node:url"

/** @typedef {import("./nessa-ui-paths.d.mts").NessaUiPath} NessaUiPath */

/** @type {readonly NessaUiPath[]} */
export const nessaUiPaths = [
  {
    specifier: "@nessa-ui/react/app-shell",
    directory: "composites/app-shell",
    whole: true,
  },
  { specifier: "@nessa-ui/react/lib/", directory: "lib/" },
  { specifier: "@nessa-ui/react/", directory: "components/" },
  { specifier: "@/components/", directory: "components/" },
  { specifier: "@/lib/", directory: "lib/" },
  { specifier: "@/provider/", directory: "provider/" },
]

/**
 * The other entries `tsconfig.json`'s `paths` holds, and why: the vendored
 * checkout carries its own `@types/react`, so without these TypeScript reads
 * React's types twice — once for the app, once for the design system's source
 * — and the two do not assign to each other (155 errors when removed). They
 * are TypeScript's half of Vite's `dedupe: ["react", "react-dom"]`.
 */
export const sharedTypes = {
  react: ["./node_modules/@types/react"],
  "react-dom": ["./node_modules/@types/react-dom"],
}

/**
 * The package's `src/` as its `node_modules` link reaches it, from the project
 * root: where `tsconfig.json` and Vitest find the source. Vite reaches it by
 * its real path instead (see `vite.config.ts`).
 */
export const linkedSourceRoot = "./node_modules/@nessa-ui/react/src"

/** Whole specifiers first, then longer prefixes before the prefixes they extend. */
const mostSpecificFirst = (paths) =>
  [...paths].sort(
    (a, b) =>
      Number(Boolean(b.whole)) - Number(Boolean(a.whole)) ||
      b.specifier.length - a.specifier.length,
  )

const escapeRegExp = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")

/**
 * The aliases for Vite's (and so Vitest's) `resolve.alias`, against
 * `sourceRoot`, the package's `src/` as that tool should reach it.
 *
 * @param {string} sourceRoot
 * @param {readonly NessaUiPath[]} [paths]
 * @returns {{ find: string | RegExp, replacement: string }[]}
 */
export function viteAliases(sourceRoot, paths = nessaUiPaths) {
  return mostSpecificFirst(paths).map(({ specifier, directory, whole }) => ({
    find: new RegExp(`^${escapeRegExp(specifier)}${whole ? "$" : ""}`),
    replacement: `${sourceRoot}/${directory}`,
  }))
}

/**
 * `tsconfig.json`'s `compilerOptions.paths`, whole: the table's entries (a
 * prefix `p` becomes `p*`), then `sharedTypes`.
 *
 * @param {readonly NessaUiPath[]} [paths]
 * @returns {Record<string, string[]>}
 */
export function tsconfigPaths(paths = nessaUiPaths) {
  const table = Object.fromEntries(
    mostSpecificFirst(paths).map(({ specifier, directory, whole }) =>
      whole
        ? [specifier, [`${linkedSourceRoot}/${directory}`]]
        : [`${specifier}*`, [`${linkedSourceRoot}/${directory}*`]],
    ),
  )
  return { ...table, ...sharedTypes }
}

const regenerate =
  "run `pnpm ui:paths`, which writes them from scripts/nessa-ui-paths.mjs"

/**
 * How `tsconfig.json`'s `compilerOptions.paths` differs from `tsconfigPaths()`:
 * an entry missing, pointing elsewhere, or there that it does not write — any
 * key at all, so an alias only TypeScript knew cannot pass unnoticed. Empty
 * when they are the same.
 *
 * @param {Record<string, unknown> | undefined} actual
 * @param {readonly NessaUiPath[]} [paths]
 * @returns {string[]}
 */
export function tsconfigPathViolations(actual, paths = nessaUiPaths) {
  const expected = tsconfigPaths(paths)
  const given = actual ?? {}
  const violations = []
  for (const [key, targets] of Object.entries(expected)) {
    const want = JSON.stringify(targets)
    if (!Object.hasOwn(given, key)) {
      violations.push(
        `tsconfig.json paths has no "${key}" (wants ${want}); ${regenerate}`,
      )
    } else if (JSON.stringify(given[key]) !== want) {
      violations.push(
        `tsconfig.json paths maps "${key}" to ${JSON.stringify(given[key])}, not ${want}; ${regenerate}`,
      )
    }
  }
  for (const key of Object.keys(given)) {
    if (!Object.hasOwn(expected, key))
      violations.push(
        `tsconfig.json paths has "${key}", which scripts/nessa-ui-paths.mjs does not write; add it there, or run `pnpm ui:paths` to drop it`,
      )
  }
  return violations
}

/**
 * `tsconfigPathViolations` for `tsconfig.json`'s text. The check reads it with
 * `JSON.parse`, having no parser for comments or trailing commas, so a file
 * that is not plain JSON is reported as such rather than thrown.
 *
 * @param {string} text
 * @param {readonly NessaUiPath[]} [paths]
 * @returns {string[]}
 */
export function tsconfigTextViolations(text, paths = nessaUiPaths) {
  let tsconfig
  try {
    tsconfig = JSON.parse(text)
  } catch (error) {
    return [
      `tsconfig.json is not plain JSON (${error.message}); the check that holds its paths to scripts/nessa-ui-paths.mjs reads it with JSON.parse, so keep it free of comments and trailing commas`,
    ]
  }
  return tsconfigPathViolations(tsconfig?.compilerOptions?.paths, paths)
}

/**
 * `tsconfig.json`'s text with its `paths` written from the table and the rest
 * kept as it was, formatted as `JSON.stringify` with two spaces.
 *
 * @param {string} text
 * @param {readonly NessaUiPath[]} [paths]
 * @returns {string}
 */
export function withTsconfigPaths(text, paths = nessaUiPaths) {
  const tsconfig = JSON.parse(text)
  tsconfig.compilerOptions = { ...tsconfig.compilerOptions, paths: tsconfigPaths(paths) }
  return `${JSON.stringify(tsconfig, null, 2)}\n`
}

// `pnpm ui:paths`: write `tsconfig.json`'s paths from the table.
const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked && process.argv.includes("--write")) {
  const file = fileURLToPath(new URL("../tsconfig.json", import.meta.url))
  writeFileSync(file, withTsconfigPaths(readFileSync(file, "utf8")))
}
