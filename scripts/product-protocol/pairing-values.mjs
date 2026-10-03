/** Compile the pairing value owner's publication into each wire consumer. */
const owners = {
  InvitationId: "identityBytes",
  ConsentIntentId: "identityBytes",
  AttemptId: "identityBytes",
  DeviceKey: "deviceKeyBytes",
}

export function derivePairingValues(schema, values) {
  for (const name of ["identityBytes", "deviceKeyBytes", "manualCodeBytes"]) {
    if (!Number.isSafeInteger(values[name]) || values[name] <= 0)
      throw new Error(`Invalid pairing owner publication: ${name}`)
  }
  const compiled = structuredClone(schema)
  function visit(node) {
    if (!node || typeof node !== "object") return
    if (Object.hasOwn(node, "x-pairing-byte-array")) {
      const name = node["x-pairing-byte-array"]
      if (!Object.hasOwn(owners, name))
        throw new Error(`Unknown pairing value owner: ${name}`)
      node.type = "array"
      node.items = { type: "integer", minimum: 0, maximum: 255 }
      node.minItems = node.maxItems = values[owners[name]]
    }
    for (const value of Object.values(node)) visit(value)
  }
  visit(compiled)
  return {
    schema: compiled,
    rust: `//! Generated from wire-values.json by generate-product-protocol.mjs.\n\npub(super) const IDENTITY_BYTES: usize = ${values.identityBytes};\npub(super) const DEVICE_KEY_BYTES: usize = ${values.deviceKeyBytes};\npub(crate) const MANUAL_CODE_BYTES: usize = ${values.manualCodeBytes};\n`,
  }
}

export function pairingValueSchema(values) {
  const derived = derivePairingValues(
    {
      $schema: "https://json-schema.org/draft/2020-12/schema",
      $id: "nessa://product/pairing-values.generated.json",
      description:
        "Generated from the pairing value owner's wire-values.json. Do not edit.",
      $defs: Object.fromEntries(
        Object.keys(owners).map((name) => [
          name,
          {
            "x-pairing-byte-array": name,
          },
        ]),
      ),
    },
    values,
  )
  derived.schema.$defs.ManualCodeDisplay = {
    type: "string",
    minLength: values.manualCodeBytes + 1,
    maxLength: values.manualCodeBytes + 1,
  }
  return derived
}

export function pairingArrayOwner(node) {
  const name = node["x-pairing-byte-array"]
  if (name === undefined) return undefined
  if (!Object.hasOwn(owners, name))
    throw new Error(`Unknown pairing value owner: ${name}`)
  return `nessa_auth::domain::pairing::${name}`
}
