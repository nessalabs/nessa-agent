/** Admission owner for the deliberately small current wire-schema grammar. */
const annotations = ["description", "x-rust-type"]
const owns = (value, key) => Object.hasOwn(value, key)
const record = (value) =>
  value !== null &&
  typeof value === "object" &&
  !Array.isArray(value) &&
  [null, Object.prototype].includes(Object.getPrototypeOf(value))
const refuse = (detail) => {
  throw new Error(`Unsupported wire schema: ${detail}`)
}
const natural = (value) => Number.isSafeInteger(value) && value >= 0
function literal(value) {
  if (typeof value !== "string") refuse("string literal")
  for (const character of value) {
    const code = character.codePointAt(0)
    if (code >= 0xd800 && code <= 0xdfff) refuse("unpaired string surrogate")
  }
  return value
}
function fields(node, accepted) {
  const allowed = new Set([...annotations, ...accepted])
  for (const key of Object.keys(node)) if (!allowed.has(key)) refuse(`keyword ${key}`)
  for (const key of annotations)
    if (owns(node, key) && typeof node[key] !== "string") refuse(`annotation ${key}`)
}
function bounds(node, minimum, maximum, requireMaximum = false) {
  for (const key of [minimum, maximum])
    if (owns(node, key) && !natural(node[key])) refuse(`bound ${key}`)
  if (requireMaximum && !owns(node, maximum)) refuse(`missing ${maximum}`)
  if (owns(node, minimum) && owns(node, maximum) && node[minimum] > node[maximum])
    refuse("reversed bounds")
  return {
    minimum: owns(node, minimum) ? node[minimum] : undefined,
    maximum: owns(node, maximum) ? node[maximum] : undefined,
  }
}
export function wireFunctionName(name) {
  if (typeof name !== "string" || !/^[A-Z][A-Za-z0-9]*$/.test(name))
    refuse("definition identifier")
  return `wire_shape_${name.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`).slice(1)}`
}
/** Fully admit the reachable graph before a caller emits any output. */
export function admitWireSchemas(definitions, roots, references = {}) {
  if (
    definitions === null ||
    typeof definitions !== "object" ||
    Array.isArray(definitions) ||
    !Array.isArray(roots) ||
    references === null ||
    typeof references !== "object" ||
    Array.isArray(references)
  )
    refuse("definition/registry inputs")
  const admitted = new Map(),
    active = new Set()
  function definition(name) {
    if (!owns(definitions, name)) throw new Error(`Unknown owned wire schema ${name}`)
    // The accepted name alphabet excludes underscores; capital markers preserve identity.
    wireFunctionName(name)
    if (active.has(name)) refuse("cyclic reference")
    if (admitted.has(name)) return
    active.add(name)
    const node = admit(definitions[name])
    active.delete(name)
    admitted.set(name, node)
  }
  function admit(node) {
    if (!record(node)) refuse("non-record node")
    if (owns(node, "$ref")) {
      fields(node, ["$ref"])
      if (typeof node.$ref !== "string" || node.$ref.length === 0)
        refuse("reference value")
      const target = node.$ref.startsWith("#/$defs/")
        ? node.$ref.slice(8)
        : owns(references, node.$ref)
          ? references[node.$ref]
          : undefined
      if (target === undefined) refuse("foreign reference")
      definition(target)
      return { kind: "reference", name: target }
    }
    if (!owns(node, "type")) {
      if (owns(node, "tsType")) {
        fields(node, ["tsType"])
        if (node.tsType !== "unknown") refuse("unknown payload annotation")
        return { kind: "any" }
      }
      if (owns(node, "const")) {
        fields(node, ["const"])
        return { kind: "stringConstant", value: literal(node.const) }
      }
      refuse("untyped node")
    }
    if (Array.isArray(node.type)) {
      fields(node, ["type", "minimum", "maximum"])
      if (node.type.length !== 2 || node.type[0] !== "integer" || node.type[1] !== "null")
        refuse("type union")
      return { kind: "nullableInteger", ...integer(node) }
    }
    switch (node.type) {
      case "string":
        fields(node, ["type", "minLength", "maxLength"])
        return { kind: "string", ...bounds(node, "minLength", "maxLength") }
      case "boolean":
        fields(node, ["type"])
        return { kind: "boolean" }
      case "integer": {
        if (owns(node, "const")) {
          fields(node, ["type", "const"])
          if (!natural(node.const)) refuse("integer constant")
          return { kind: "integerConstant", value: node.const }
        }
        fields(node, ["type", "minimum", "maximum"])
        return { kind: "integer", ...integer(node) }
      }
      case "array": {
        fields(node, ["type", "items", "minItems", "maxItems"])
        if (!owns(node, "items")) refuse("missing items")
        return {
          kind: "array",
          ...bounds(node, "minItems", "maxItems", true),
          items: admit(node.items),
        }
      }
      case "object": {
        fields(node, ["type", "properties", "required", "additionalProperties"])
        if (
          !owns(node, "properties") ||
          !record(node.properties) ||
          !owns(node, "required") ||
          !Array.isArray(node.required) ||
          !owns(node, "additionalProperties") ||
          node.additionalProperties !== false
        )
          refuse("closed object declaration")
        const required = new Set()
        for (const key of node.required) {
          literal(key)
          if (!owns(node.properties, key) || required.has(key))
            refuse("required property membership")
          required.add(key)
        }
        return {
          kind: "object",
          properties: Object.entries(node.properties).map(([name, child]) => ({
            name: literal(name),
            required: required.has(name),
            node: admit(child),
          })),
        }
      }
      default:
        refuse("type")
    }
  }
  function integer(node) {
    if (!owns(node, "minimum") || node.minimum !== 0) refuse("unsigned minimum")
    if (owns(node, "maximum") && !natural(node.maximum)) refuse("integer maximum")
    return { maximum: owns(node, "maximum") ? node.maximum : undefined }
  }
  for (const root of roots) definition(root)
  return admitted
}
