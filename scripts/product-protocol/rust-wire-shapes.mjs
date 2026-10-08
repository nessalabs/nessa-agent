import { admitWireSchemas, wireFunctionName } from "./wire-shape-schema.mjs"
/** Encode already-admitted Unicode into Rust, including control characters. */
function rustString(value) {
  return (
    '"' +
    [...value]
      .map((character) => {
        if (character === '"' || character === "\\") return "\\" + character
        const code = character.codePointAt(0)
        return code < 32 || code === 127 ? `\\u{${code.toString(16)}}` : character
      })
      .join("") +
    '"'
  )
}
/** Emit only the admitted grammar; raw schema decisions belong to admission. */
export function rustWireShapes(definitions, roots, references = {}) {
  const admitted = admitWireSchemas(definitions, roots, references)
  function unsigned(node, value) {
    return node.maximum === undefined
      ? `${value}.as_u64().is_some()`
      : `${value}.as_u64().is_some_and(|number| number <= ${node.maximum})`
  }
  function shape(node, value) {
    switch (node.kind) {
      case "any":
        return "true"
      case "reference":
        return `${wireFunctionName(node.name)}(${value})`
      case "stringConstant":
        return `${value}.as_str() == Some(${rustString(node.value)})`
      case "integerConstant":
        return `${value}.as_u64() == Some(${node.value})`
      case "boolean":
        return `${value}.is_boolean()`
      case "integer":
        return unsigned(node, value)
      case "nullableInteger":
        return `${value}.is_null() || ${unsigned(node, value)}`
      case "stringEnum":
        return `${value}.as_str().is_some_and(|text| [${node.values.map(rustString).join(",")}].contains(&text))`
      case "string": {
        const conditions = []
        if (node.minimum !== undefined)
          conditions.push(`text.chars().count() >= ${node.minimum}`)
        if (node.maximum !== undefined)
          conditions.push(`text.chars().count() <= ${node.maximum}`)
        return conditions.length
          ? `${value}.as_str().is_some_and(|text| ${conditions.join(" && ")})`
          : `${value}.is_string()`
      }
      case "array": {
        const conditions = [`items.len() <= ${node.maximum}`]
        if (node.minimum !== undefined) conditions.push(`items.len() >= ${node.minimum}`)
        conditions.push(
          node.items.kind === "reference"
            ? `items.iter().all(${wireFunctionName(node.items.name)})`
            : `items.iter().all(|item| { let _ = item; ${shape(node.items, "item")} })`,
        )
        return `${value}.as_array().is_some_and(|items| ${conditions.join(" && ")})`
      }
      case "object": {
        if (!node.properties.length)
          return `${value}.as_object().is_some_and(|object| object.is_empty())`
        const conditions = node.properties.map(
          (property) =>
            `object.get(${rustString(property.name)}).${property.required ? "is_some_and" : "is_none_or"}(|field| { let _ = field; ${shape(property.node, "field")} })`,
        )
        conditions.push(
          `object.keys().all(|key| [${node.properties.map((property) => rustString(property.name)).join(",")}].contains(&key.as_str()))`,
        )
        return `${value}.as_object().is_some_and(|object| ${conditions.join(" && ")})`
      }
      default:
        throw new Error("Unknown admitted wire schema form")
    }
  }
  return (
    [...admitted]
      .map(
        ([name, node]) =>
          `pub fn ${wireFunctionName(name)}(value: &Value) -> bool { ${shape(node, "value")} }`,
      )
      .join("\n") + "\n"
  )
}
