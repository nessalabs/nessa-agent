/** Types for `nessa-ui-paths.mjs`, for the TypeScript configs that import it. */

/**
 * One rule of the table: an import prefix to a directory of the package's
 * `src/`, or with `whole`, one exact specifier to one path.
 */
export interface NessaUiPath {
  readonly specifier: string
  readonly directory: string
  readonly whole?: true
}

export const nessaUiPaths: readonly NessaUiPath[]
export const linkedSourceRoot: string
export const sharedPackages: readonly string[]

export function viteAliases(
  sourceRoot: string,
  paths?: readonly NessaUiPath[],
): { find: RegExp; replacement: string }[]

export function tsconfigPaths(paths?: readonly NessaUiPath[]): Record<string, string[]>

export function tsconfigPathViolations(
  actual: Record<string, unknown> | undefined,
  paths?: readonly NessaUiPath[],
): string[]

export function tsconfigTextViolations(
  text: string,
  paths?: readonly NessaUiPath[],
): string[]

export function withTsconfigPaths(text: string, paths?: readonly NessaUiPath[]): string
