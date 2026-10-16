/** Publish the event schema's conditional metadata contract for runtime clients. */
export function eventMetadataRules(schema) {
  const event = schema.$defs.ConversationEvent
  const kinds = schema.$defs.ConversationEventKind.enum
  const seen = new Set()
  const rules = []
  const only = (value, keys) =>
    value &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    Object.keys(value).every((key) => keys.includes(key))
  for (const condition of event.allOf ?? []) {
    if (
      !only(condition, ["if", "then"]) ||
      !only(condition.if, ["properties"]) ||
      !only(condition.if.properties, ["kind"]) ||
      !only(condition.if.properties.kind, ["const"])
    )
      throw new Error("Unsupported semantic event condition contract")
    const kind = condition.if?.properties?.kind?.const
    if (!kinds.includes(kind) || seen.has(kind) || !condition.then)
      throw new Error("Invalid semantic event kind contract")
    seen.add(kind)
    if (
      Object.hasOwn(condition.then, "oneOf") &&
      (!only(condition.then, ["oneOf"]) ||
        !Array.isArray(condition.then.oneOf) ||
        condition.then.oneOf.length === 0)
    )
      throw new Error("Unsupported semantic event alternative contract")
    for (const shape of condition.then.oneOf ?? [condition.then]) {
      if (
        Object.keys(shape).some((key) => !["required", "properties", "not"].includes(key))
      )
        throw new Error("Unsupported semantic event metadata contract")
      const required = shape.required ?? []
      if (!Array.isArray(required))
        throw new Error("Invalid required semantic event metadata")
      if (
        shape.not &&
        (!only(shape.not, ["anyOf"]) ||
          !Array.isArray(shape.not.anyOf) ||
          shape.not.anyOf.length === 0)
      )
        throw new Error("Unsupported forbidden semantic event metadata")
      for (const property of Object.values(shape.properties ?? {})) {
        if (
          Object.keys(property).length !== 1 ||
          (!Object.hasOwn(property, "const") && !Array.isArray(property.enum))
        )
          throw new Error("Unsupported semantic event metadata values")
      }
      const forbidden = (shape.not?.anyOf ?? []).map((entry) => {
        if (Object.keys(entry).length !== 1 || entry.required?.length !== 1)
          throw new Error("Invalid forbidden semantic event metadata")
        return entry.required[0]
      })
      const values = {}
      for (const [field, property] of Object.entries({
        ...event.properties,
        ...shape.properties,
      })) {
        if (Object.hasOwn(property, "const")) values[field] = [property.const]
        else if (property.enum) values[field] = property.enum
      }
      for (const field of [...required, ...forbidden, ...Object.keys(values)]) {
        if (!Object.hasOwn(event.properties, field))
          throw new Error("Unknown semantic event metadata field")
      }
      rules.push({ kind, required, forbidden, values })
    }
  }
  if (seen.size !== kinds.length)
    throw new Error("Every semantic event kind needs a metadata contract")
  return rules
}
