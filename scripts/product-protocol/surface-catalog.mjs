/**
 * The CLI and MCP surface of the product manifest.
 *
 * One list. `nessa call` and `nessa mcp` both expose `methods` and neither
 * exposes `excluded`. A manifest method that is in neither fails the build:
 * it would otherwise be an app-only action with no written reason.
 */

/**
 * @param {unknown} value
 * @returns {value is Record<string, unknown>}
 */
function record(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value)
}

/**
 * @param {unknown} exclusions
 * @param {Record<string, unknown>} methods
 */
function excludedMethods(exclusions, methods) {
  if (!record(exclusions))
    throw new Error("Surface exclusions must be an object of method to reason")
  /** @type {{ method: string, reason: string }[]} */
  const excluded = []
  for (const [method, reason] of Object.entries(exclusions)) {
    if (!Object.hasOwn(methods, method))
      throw new Error(`Surface exclusion ${method} is not a manifest method`)
    if (typeof reason !== "string" || reason.trim() === "")
      throw new Error(`Surface exclusion ${method} needs a written reason`)
    excluded.push({ method, reason })
  }
  return excluded
}

/**
 * @param {unknown} hints
 * @param {string} method
 */
function hint(hints, method) {
  if (!record(hints) || !Object.hasOwn(hints, method))
    throw new Error(
      `Manifest method ${method} has no CLI or MCP entry and is not excluded`,
    )
  const entry = hints[method]
  if (
    !record(entry) ||
    typeof entry.readOnly !== "boolean" ||
    typeof entry.destructive !== "boolean" ||
    Object.keys(entry).some((key) => key !== "readOnly" && key !== "destructive")
  )
    throw new Error(
      `Surface hint ${method} must be { readOnly, destructive }, both booleans`,
    )
  if (entry.readOnly && entry.destructive)
    throw new Error(`Surface hint ${method} cannot be read-only and destructive`)
  return { readOnly: entry.readOnly, destructive: entry.destructive }
}

/**
 * Build the surface catalog, or throw when a manifest method is neither
 * exposed nor excluded with a reason.
 *
 * @param {{
 *   manifest: { methods?: unknown },
 *   schema: { $defs?: Record<string, unknown> },
 *   exclusions: unknown,
 *   hints: unknown,
 * }} input
 */
export function buildSurfaceCatalog(input) {
  const methods = input.manifest?.methods
  if (!record(methods)) throw new Error("Product manifest methods must be an object")
  const defs = input.schema?.$defs
  if (!record(defs)) throw new Error("Product schema has no definitions")
  const excluded = excludedMethods(input.exclusions, methods)
  const excludedNames = new Set(excluded.map((entry) => entry.method))
  if (!record(input.hints)) throw new Error("Surface hints must be an object")
  for (const method of Object.keys(input.hints)) {
    if (!Object.hasOwn(methods, method))
      throw new Error(`Surface hint ${method} is not a manifest method`)
    if (excludedNames.has(method))
      throw new Error(`Surface hint ${method} is excluded from CLI and MCP`)
  }
  const exposed = []
  for (const [name, spec] of Object.entries(methods)) {
    if (excludedNames.has(name)) continue
    if (!record(spec) || !Object.hasOwn(spec, "params") || !Object.hasOwn(spec, "grant"))
      throw new Error(`Product method ${name} must declare params and a grant`)
    const params = spec.params
    const grant = spec.grant
    if (params !== null && (typeof params !== "string" || !Object.hasOwn(defs, params)))
      throw new Error(`Product method ${name} params must name a schema or be null`)
    if (grant !== null && typeof grant !== "string")
      throw new Error(`Product method ${name} has no declared grant`)
    const { readOnly, destructive } = hint(input.hints, name)
    exposed.push({
      name,
      params,
      grant,
      readOnly,
      destructive,
    })
  }
  return { excluded, methods: exposed }
}
