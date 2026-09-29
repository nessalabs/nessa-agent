/**
 * What a source file imports, as the specifiers it names — every way a
 * module can be reached from text: a named import or a re-export, a
 * side-effect import (a stylesheet), a dynamic import, and a test's
 * `vi.mock`, which stands in for the module it names.
 *
 * Pure text, like every rule here: `check-architecture.mjs` runs on bare Node.
 * It reads string literals only; a specifier built at runtime is not an
 * import it can see.
 */
const forms = [
  // a named import, a type import, a re-export
  /\bfrom\s+["']([^"']+)["']/g,
  // a side-effect import: a module run for what it does, a stylesheet among them
  /(?:^|[;\s])import\s+["']([^"']+)["']/g,
  // a dynamic import
  /\bimport\s*\(\s*["']([^"']+)["']/g,
  // vi.mock and vi.doMock
  /\bvi\.(?:do)?[mM]ock\s*\(\s*["']([^"']+)["']/g,
]

export function importedPaths(text) {
  return forms.flatMap((form) => [...text.matchAll(form)].map((match) => match[1]))
}
