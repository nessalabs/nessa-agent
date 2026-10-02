/**
 * Where an import of the design system lands, for every tool that resolves
 * one: the one table, and the owner of all of `tsconfig.json`'s `paths`.
 *
 * Nessa UI is consumed as source (see `vite.config.ts` for why), so three
 * tools each resolve its import paths: TypeScript (`tsconfig.json`'s
 * `paths`), Vite, and Vitest. They were three hand-kept copies, and they had
 * already parted: Vitest sent `@nessa-ui/react/app-shell` to a components
 * directory that does not exist. Now Vite and Vitest build their aliases from
 * this table. `tsconfig.json` cannot import code, so its `paths` are written
 * from here (`pnpm ui:paths`, `write-nessa-ui-paths.mjs`) — the table's
 * entries and the React type redirects its source needs (`sharedPackages`),
 * and nothing else. `scripts/check-architecture.mjs` fails when any entry is
 * missing, points elsewhere, or is not one of those, or when `baseUrl` or
 * `extends` would move them, or the file is not a plain JSON object
 * (`tsconfigTextViolations`); the test "the repository's tsconfig.json is
 * exactly what the writer makes of it" holds the file byte for byte. An alias
 * of the app's own would be a new decision: it is added here, or this module
 * stops owning `paths` whole.
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
 * `node_modules`, so it imports nothing; that check's import rule reads this
 * file's own imports (not what they import in turn). Its types are
 * `nessa-ui-paths.d.mts`'s.
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
 * The packages the app and the design system's source must share one copy of.
 * The vendored checkout carries its own, so Vite and Vitest `dedupe` them, and
 * `tsconfig.json`'s `paths` points each, and every subpath of it
 * (`react/jsx-runtime`, which every TSX file imports), at this app's
 * `@types/<name>`: without that TypeScript reads React's types twice — once
 * for the app, once for the design system's source — and the two do not
 * assign to each other (155 errors when the bare names are removed). The
 * subpath redirect also covers `react/package.json`, which would then type
 * against `@types/react`'s; nothing imports it.
 */
export const sharedPackages = ["react", "react-dom"]

/** `tsconfig.json`'s redirect of each shared package, and its subpaths, to this app's types. */
const sharedTypes = Object.fromEntries(
  sharedPackages.flatMap((name) => [
    [name, [`./node_modules/@types/${name}`]],
    [`${name}/*`, [`./node_modules/@types/${name}/*`]],
  ]),
)

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
 * @returns {{ find: RegExp, replacement: string }[]}
 */
export function viteAliases(sourceRoot, paths = nessaUiPaths) {
  return mostSpecificFirst(paths).map(({ specifier, directory, whole }) => ({
    find: new RegExp(`^${escapeRegExp(specifier)}${whole ? "$" : ""}`),
    // Vite applies this with String.replace, where `$` is special.
    replacement: `${sourceRoot}/${directory}`.replaceAll("$", "$$$$"),
  }))
}

/**
 * `tsconfig.json`'s `compilerOptions.paths`, whole: the table's entries (a
 * prefix `p` becomes `p*`), then each shared package's types.
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
        `tsconfig.json paths has "${key}", which scripts/nessa-ui-paths.mjs does not write — it owns paths whole; add the entry there, or run \`pnpm ui:paths\` to drop it`,
      )
  }
  return violations
}

const isObject = (value) =>
  value !== null && typeof value === "object" && !Array.isArray(value)

const plainJson =
  "scripts/nessa-ui-paths.mjs reads and writes it with JSON, so keep it a JSON object free of comments and trailing commas"

/**
 * `tsconfig.json`'s text read as the object the table writes into, with the
 * byte-order mark it began with, if any (TypeScript accepts one) — or a
 * sentence saying why it cannot be: not JSON, not an object, or a
 * `compilerOptions` that is not one.
 *
 * @param {string} text
 * @returns {{ tsconfig: Record<string, unknown>, bom: string } | { problem: string }}
 */
function readTsconfig(text) {
  const bom = text.startsWith("\uFEFF") ? "\uFEFF" : ""
  let tsconfig
  try {
    tsconfig = JSON.parse(text.slice(bom.length))
  } catch (error) {
    // The parser can quote the source, newlines and all; the refusal stays one line.
    const why = error.message.replace(/\s+/g, " ")
    return { problem: `tsconfig.json is not plain JSON (${why}); ${plainJson}` }
  }
  if (!isObject(tsconfig))
    return { problem: `tsconfig.json is not a JSON object; ${plainJson}` }
  if (Object.hasOwn(tsconfig, "compilerOptions") && !isObject(tsconfig.compilerOptions))
    return { problem: `tsconfig.json's compilerOptions is not an object; ${plainJson}` }
  return { tsconfig, bom }
}

/**
 * `tsconfigPathViolations` for `tsconfig.json`'s text, and the two settings
 * that would move what it holds: `baseUrl` re-roots every path, and `extends`
 * can bring one in. Text that is not a plain JSON object — the check reads it
 * with `JSON.parse`, having no parser for comments or trailing commas — is
 * reported as such rather than thrown.
 *
 * @param {string} text
 * @param {readonly NessaUiPath[]} [paths]
 * @returns {string[]}
 */
export function tsconfigTextViolations(text, paths = nessaUiPaths) {
  const read = readTsconfig(text)
  if ("problem" in read) return [read.problem]
  const { tsconfig } = read
  const options = isObject(tsconfig.compilerOptions) ? tsconfig.compilerOptions : {}
  const moved = []
  if (Object.hasOwn(tsconfig, "extends"))
    moved.push(
      "tsconfig.json has `extends`, which can bring in a baseUrl or paths that scripts/nessa-ui-paths.mjs does not write; the paths it writes are relative to tsconfig.json alone",
    )
  if (Object.hasOwn(options, "baseUrl"))
    moved.push(
      "tsconfig.json has `baseUrl`, which re-roots every path scripts/nessa-ui-paths.mjs writes; they are relative to tsconfig.json",
    )
  return [...moved, ...tsconfigPathViolations(options.paths, paths)]
}

/**
 * `tsconfig.json`'s text with its `paths` written from the table and the rest
 * kept as it was, formatted as `JSON.stringify` with two spaces: CRLF line
 * endings if the text had any, LF otherwise, and its byte-order mark if it
 * had one. Throws a sentence, not a parser's error, when the text is not a
 * plain JSON object.
 *
 * @param {string} text
 * @param {readonly NessaUiPath[]} [paths]
 * @returns {string}
 */
export function withTsconfigPaths(text, paths = nessaUiPaths) {
  const read = readTsconfig(text)
  if ("problem" in read) throw new Error(read.problem)
  const { tsconfig, bom } = read
  tsconfig.compilerOptions = { ...tsconfig.compilerOptions, paths: tsconfigPaths(paths) }
  const eol = text.includes("\r\n") ? "\r\n" : "\n"
  return bom + `${JSON.stringify(tsconfig, null, 2)}\n`.replaceAll("\n", eol)
}

/** What stopped a read or write, as one short phrase, whatever was thrown. */
function failure(error) {
  let phrase
  try {
    phrase = String(error?.code ?? error?.message ?? error)
  } catch {
    phrase = "an error with no description"
  }
  return phrase.replace(/\s+/g, " ")
}

/**
 * What `pnpm ui:paths` does to `file`, given how to read and write it, so
 * every way it can go is decided here and tested without a filesystem: the
 * bytes must be UTF-8 and a plain JSON object; the file is written only when
 * that changes it. Each refusal is one line naming the file. A refusal before
 * the write leaves the file as it was. The write itself is in place: one that
 * fails partway (a full disk) can leave the file partly written, and its
 * refusal says so and how to recover — `tsconfig.json` is tracked, so git
 * restores it. Writing in place keeps the file what it is (its mode, a symlink
 * to it, its links), which a temporary file renamed over it would not.
 *
 * @param {string} file
 * @param {{ read: (file: string) => Uint8Array, write: (file: string, text: string) => void }} io
 * @param {readonly NessaUiPath[]} [paths]
 * @returns {{ written: boolean } | { refused: string }}
 */
export function writeTsconfigPaths(file, { read, write }, paths = nessaUiPaths) {
  let bytes
  try {
    bytes = read(file)
  } catch (error) {
    return { refused: `${file} cannot be read (${failure(error)})` }
  }
  let before
  try {
    // ignoreBOM keeps a byte-order mark in the text, so the writer can keep it.
    before = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes)
  } catch {
    return { refused: `${file} is not UTF-8 text; it is left as it was` }
  }
  let after
  try {
    after = withTsconfigPaths(before, paths)
  } catch (error) {
    return { refused: `${file}: ${error.message}` }
  }
  if (after === before) return { written: false }
  try {
    write(file, after)
  } catch (error) {
    return {
      refused: `${file} cannot be written (${failure(error)}); if it was left partly written, restore it with \`git checkout -- ${file}\` and run \`pnpm ui:paths\` again`,
    }
  }
  return { written: true }
}
