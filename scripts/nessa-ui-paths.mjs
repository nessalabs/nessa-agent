/**
 * Where an import of the design system lands in its source: the one table.
 *
 * Nessa UI is consumed as source (see `vite.config.ts` for why), so three
 * tools each resolve its import paths: TypeScript (`tsconfig.json`'s
 * `paths`), Vite, and Vitest. They were three hand-kept copies, and they had
 * already parted: Vitest sent `@nessa-ui/react/app-shell` to a components
 * directory that does not exist. Now Vite and Vitest build their aliases from
 * this table, and `scripts/check-architecture.mjs` fails when `tsconfig.json`,
 * which cannot import code, disagrees with it (`tsconfigPathViolations`).
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
 * components' rule. Pure on purpose — the architecture check imports this with
 * bare Node and no `node_modules`. Its types are `nessa-ui-paths.d.mts`'s.
 */

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
 * The entries `tsconfig.json`'s `compilerOptions.paths` must hold for the
 * table: a prefix `p` becomes `p*`.
 *
 * @param {readonly NessaUiPath[]} [paths]
 * @returns {Record<string, string[]>}
 */
export function tsconfigPaths(paths = nessaUiPaths) {
  return Object.fromEntries(
    mostSpecificFirst(paths).map(({ specifier, directory, whole }) =>
      whole
        ? [specifier, [`${linkedSourceRoot}/${directory}`]]
        : [`${specifier}*`, [`${linkedSourceRoot}/${directory}*`]],
    ),
  )
}

/**
 * Whether a `paths` entry leads into the design system's source, which makes
 * it the table's to hold, whatever its key.
 */
const intoTheSource = (targets) =>
  Array.isArray(targets) &&
  targets.some(
    (target) => typeof target === "string" && target.startsWith(`${linkedSourceRoot}/`),
  )

/**
 * How `tsconfig.json`'s `compilerOptions.paths` disagrees with the table: an
 * entry missing, pointing elsewhere, or leading into the design system's
 * source under a key the table does not have. Entries that lead elsewhere
 * (`react`, `react-dom`) are not the table's and are left alone. Empty when
 * they agree.
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
        `tsconfig.json paths has no "${key}"; scripts/nessa-ui-paths.mjs maps it to ${want}`,
      )
    } else if (JSON.stringify(given[key]) !== want) {
      violations.push(
        `tsconfig.json paths maps "${key}" to ${JSON.stringify(given[key])}; scripts/nessa-ui-paths.mjs maps it to ${want}`,
      )
    }
  }
  for (const [key, targets] of Object.entries(given)) {
    if (intoTheSource(targets) && !Object.hasOwn(expected, key))
      violations.push(
        `tsconfig.json paths has "${key}", which scripts/nessa-ui-paths.mjs does not map`,
      )
  }
  return violations
}
