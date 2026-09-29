/**
 * What a source file imports, as the specifiers it names — every way a
 * module can be reached from text: a named import or a re-export, a
 * side-effect import (a stylesheet), a dynamic import, and a test's
 * `vi.mock`, `vi.doMock`, `vi.importActual` and `vi.importMock`, which stand
 * in for, or reach, the module they name. Not `require()` or a path alias:
 * the source uses neither.
 *
 * Pure text, like every rule here: `check-architecture.mjs` runs on bare Node.
 * It reads string literals only; a specifier built at runtime is not an
 * import it can see. Comments are not code: they are blanked first
 * (`without-comments.mjs`), so an example written in one — a forbidden import,
 * documented — is never read as an import.
 */
import { withoutComments } from "./without-comments.mjs"

const forms = [
  // a named import, a type import, a re-export
  /\bfrom\s+["']([^"']+)["']/g,
  // a side-effect import: a module run for what it does, a stylesheet among them
  /(?:^|[;\s])import\s+["']([^"']+)["']/g,
  // a dynamic import
  /\bimport\s*\(\s*["']([^"']+)["']/g,
  // vi.mock, vi.doMock, vi.importActual, vi.importMock
  /\bvi\.(?:(?:do)?[mM]ock|importActual|importMock)\s*(?:<[^>]*>)?\s*\(\s*["']([^"']+)["']/g,
]

export function importedPaths(text) {
  const code = withoutComments(text)
  return forms.flatMap((form) => [...code.matchAll(form)].map((match) => match[1]))
}
