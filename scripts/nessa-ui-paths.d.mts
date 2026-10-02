/** Types for `nessa-ui-paths.mjs`, for the TypeScript configs that import it. */

/** One rule of the table: an import prefix (or `whole` specifier) to a directory of the package's `src/`. */
export interface NessaUiPath {
  readonly specifier: string
  readonly directory: string
  readonly whole?: true
}

export const nessaUiPaths: readonly NessaUiPath[]
export const tsconfigSourceRoot: string

export function viteAliases(
  sourceRoot: string,
  paths?: readonly NessaUiPath[],
): { find: string | RegExp; replacement: string }[]

export function tsconfigPaths(paths?: readonly NessaUiPath[]): Record<string, string[]>

export function tsconfigPathViolations(
  actual: Record<string, unknown> | undefined,
  paths?: readonly NessaUiPath[],
): string[]
