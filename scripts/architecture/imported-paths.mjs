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
 * (`withoutComments`), so an example written in one — a forbidden import,
 * documented — is never read as an import.
 */
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

/** After these, a `/` begins a regular expression, not a division. */
const beforeRegex = new Set([..."(,=:[!&|?{};+-*%<>~^"])
const regexKeywords =
  /(?:^|[^\w$])(?:return|typeof|instanceof|in|of|new|delete|void|throw|case|do|else|yield|await)$/

/**
 * `text` with every comment — line or block — turned to spaces, newlines
 * kept, so everything else stays where it was. Strings, template literals
 * (their `${…}` read as code) and regular expressions are walked over whole,
 * so a `//` or `/*` inside one is not taken for a comment.
 */
export function withoutComments(text) {
  const out = [...text]
  const blank = (from, to) => {
    for (let i = from; i < to; i++) if (out[i] !== "\n") out[i] = " "
  }
  /** How deep in `${…}` each open template literal is, innermost last. */
  const templates = []
  let i = 0
  const skipString = (quote) => {
    for (i++; i < text.length && text[i] !== quote; i++) if (text[i] === "\\") i++
    i++
  }
  /** Walks a template literal's text from `i`, stopping after its end or at a `${`. */
  const skipTemplateText = () => {
    for (; i < text.length; i++) {
      if (text[i] === "\\") {
        i++
        continue
      }
      if (text[i] === "`") {
        i++
        templates.pop()
        return
      }
      if (text[i] === "$" && text[i + 1] === "{") {
        i += 2
        templates[templates.length - 1]++
        return
      }
    }
  }
  const regexMayStart = () => {
    let j = i - 1
    while (j >= 0 && /\s/.test(text[j])) j--
    if (j < 0) return true
    return (
      beforeRegex.has(text[j]) ||
      regexKeywords.test(text.slice(Math.max(0, j - 12), j + 1))
    )
  }
  while (i < text.length) {
    const c = text[i]
    const next = text[i + 1]
    if (c === "/" && next === "/") {
      const end = text.indexOf("\n", i)
      const stop = end === -1 ? text.length : end
      blank(i, stop)
      i = stop
    } else if (c === "/" && next === "*") {
      const end = text.indexOf("*/", i + 2)
      const stop = end === -1 ? text.length : end + 2
      blank(i, stop)
      i = stop
    } else if (c === '"' || c === "'") skipString(c)
    else if (c === "`") {
      i++
      templates.push(0)
      skipTemplateText()
    } else if (c === "{" && templates.length > 0) {
      templates[templates.length - 1]++
      i++
    } else if (c === "}" && templates.length > 0) {
      templates[templates.length - 1]--
      i++
      // Back from `${…}` into the template's own text.
      if (templates[templates.length - 1] === 0) skipTemplateText()
    } else if (c === "/" && regexMayStart()) {
      let inClass = false
      for (i++; i < text.length && text[i] !== "\n"; i++) {
        if (text[i] === "\\") i++
        else if (text[i] === "[") inClass = true
        else if (text[i] === "]") inClass = false
        else if (text[i] === "/" && !inClass) break
      }
      i++
    } else i++
  }
  return out.join("")
}
