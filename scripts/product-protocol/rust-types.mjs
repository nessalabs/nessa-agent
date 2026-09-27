/** Validate external Rust imports before generating any output files. */
export function validateExternalRustTypes(definitions) {
  const names = new Map(
    Object.entries(definitions)
      .filter(([, definition]) => !definition["x-rust-type"])
      .map(([name]) => [name, `generated type ${name}`]),
  )
  for (const name of [
    "String",
    "Vec",
    "Option",
    "Method",
    "Event",
    "Serialize",
    "Deserialize",
    "u64",
    "u16",
    "bool",
  ]) {
    names.set(name, `reserved type ${name}`)
  }
  for (const [definition, node] of Object.entries(definitions)) {
    const path = node["x-rust-type"]
    if (path === undefined) continue
    if (
      typeof path !== "string" ||
      !/^[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+$/.test(path)
    ) {
      throw new Error(`${definition}: x-rust-type must be a qualified Rust type path`)
    }
    const name = path.split("::").at(-1)
    const previous = names.get(name)
    if (previous !== undefined && previous !== path) {
      throw new Error(
        `${definition}: Rust type name ${name} conflicts between ${previous} and ${path}`,
      )
    }
    names.set(name, path)
  }
}
