/** Publish the manifest's closed Rust method identity without server policy. */
export function methodIdentities(names) {
  const variants = new Map()
  return names.map((name) => {
    if (
      typeof name !== "string" ||
      !/^[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)*$/.test(name)
    )
      throw new Error(`Invalid product method Rust identity: ${JSON.stringify(name)}`)
    const variant = name
      .split(".")
      .map((part) => part[0].toUpperCase() + part.slice(1))
      .join("")
    if (variant === "Self")
      throw new Error(`Invalid product method Rust identity: ${name}`)
    if (variants.has(variant))
      throw new Error(
        `Product method Rust variant ${variant} collides: ${variants.get(variant)} and ${name}`,
      )
    variants.set(variant, name)
    return { name, variant }
  })
}

/** Render only identity and exact parsing; the gateway owns method plans. */
export function methodIdentityRust(names) {
  const methods = methodIdentities(names)
  return `/// A method declared by the product manifest; carries no admission verdict.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ProductMethod {
${methods.map(({ name, variant }) => `    /// The \`${name}\` method.\n    ${variant},`).join("\n")}
}
impl ProductMethod {
    /// The complete manifest method catalogue, including authentication.
    pub const ALL: &'static [Self] = &[${methods.map(({ variant }) => `Self::${variant}`).join(",")}];
    /// Return this identity's unchanged wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
${methods.map(({ name, variant }) => `            Self::${variant} => ${JSON.stringify(name)},`).join("\n")}
        }
    }
    /// Parse an exact, case-sensitive manifest wire name; unknown names return \`None\`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
${methods.map(({ name, variant }) => `            ${JSON.stringify(name)} => Some(Self::${variant}),`).join("\n")}
            _ => None,
        }
    }
}
`
}
