/** The phone wire envelope retains the whole original; Auth owns consent policy. */
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"
import Ajv2020 from "ajv/dist/2020.js"

const read = (name) =>
  JSON.parse(readFileSync(new URL(`../../protocol/product/${name}`, import.meta.url)))
const schema = read("v1.json")
const fixtures = read("fixtures.json")
const ajv = new Ajv2020({ strict: false, allErrors: true })
ajv.addSchema(schema)
const validate = (name, value) => ajv.getSchema(`${schema.$id}#/$defs/${name}`)(value)

test("phone wire preserves discovery, original, full consent and terminal cleanup", () => {
  for (const name of Object.keys(fixtures).filter((name) => name.startsWith("Phone")))
    assert.equal(validate(name, fixtures[name]), true, name)
  const status = fixtures.PhoneAuthorizationStatus
  assert.equal(
    validate("PhoneAuthorizationStatus", {
      ...status,
      phase: "terminal",
      terminal: { cause: "credential_revoked", initiator: { kind: "system" } },
      cleanupPending: true,
      activationStopped: "permanent",
    }),
    true,
  )
  assert.equal(
    validate("PhoneAuthorizationStatus", {
      ...status,
      phase: "approved",
      credentialId: undefined,
      receiver: undefined,
    }),
    true,
  )
})

test("phone byte fields refuse shortened, extended and non-octet evidence", () => {
  const authorization = fixtures.PhoneAuthorizeParams
  const status = fixtures.PhoneStatusParams
  const paths = [
    ["PhoneAuthorizeParams", authorization, ["signature"]],
    ["PhoneAuthorizeParams", authorization, ["original", "requestId"]],
    ["PhoneAuthorizeParams", authorization, ["original", "deviceKey"]],
    ["PhoneAuthorizeParams", authorization, ["original", "discovery", "gatewayKey"]],
    ["PhoneStatusParams", status, ["requestId"]],
    ["PhoneStatusParams", status, ["originalDigest"]],
    ["PhoneAuthorizationStatus", fixtures.PhoneAuthorizationStatus, ["invitationId"]],
    ["PhoneAuthorizationStatus", fixtures.PhoneAuthorizationStatus, ["consentId"]],
    ["PhoneAuthorizationStatus", fixtures.PhoneAuthorizationStatus, ["originalDigest"]],
  ]
  for (const [name, fixture, path] of paths) {
    const original = path.reduce((value, key) => value[key], fixture)
    for (const replacement of [
      original.slice(1),
      [...original, 0],
      original.map((byte, index) => (index ? byte : -1)),
      original.map((byte, index) => (index ? byte : 256)),
      original.map((byte, index) => (index ? byte : 1.5)),
      original.map((byte, index) => (index ? byte : "1")),
    ]) {
      const changed = structuredClone(fixture)
      const parent = path.slice(0, -1).reduce((value, key) => value[key], changed)
      parent[path.at(-1)] = replacement
      assert.equal(validate(name, changed), false, `${name}.${path.join(".")}`)
    }
  }
})

test("phone envelopes require complete originals and refuse aliases or secret fields", () => {
  const original = fixtures.PhoneOriginal
  for (const [name, fixture] of Object.entries(fixtures).filter(([name]) =>
    name.startsWith("Phone"),
  )) {
    const optional =
      name === "PhoneAuthorizationStatus" ? ["credentialId", "receiver"] : []
    for (const key of Object.keys(fixture).filter((key) => !optional.includes(key))) {
      const changed = structuredClone(fixture)
      delete changed[key]
      assert.equal(validate(name, changed), false, `${name} missing ${key}`)
    }
    for (const key of ["code", "privateKey", "token", "grant"])
      assert.equal(validate(name, { ...fixture, [key]: "untrusted" }), false, name)
  }
  for (const key of Object.keys(original.discovery.source)) {
    const changed = structuredClone(original)
    delete changed.discovery.source[key]
    assert.equal(validate("PhoneOriginal", changed), false, key)
  }
  const changed = structuredClone(original)
  changed.discovery.consent = {
    class: original.discovery.consent.class,
    grant: original.discovery.consent.grants[0],
  }
  assert.equal(validate("PhoneOriginal", changed), false)
  assert.equal(validate("PhoneEndpointParams", { endpoint: "127.0.0.1:1" }), false)
})

test("phone wire bounds grant capacity without copying Auth ordered-action policy", () => {
  const consent = fixtures.PhoneConsent
  assert.equal(validate("PhoneConsent", { ...consent, grants: [] }), false)
  const capacity = schema.$defs.ProductCredentialMetadata.properties.grants.maxItems
  assert.equal(schema.$defs.PhoneConsent.properties.grants.maxItems, capacity)
  assert.equal(
    validate("PhoneConsent", {
      ...consent,
      grants: Array(capacity).fill(consent.grants[0]),
    }),
    true,
  )
  assert.equal(
    validate("PhoneConsent", {
      ...consent,
      grants: Array(capacity + 1).fill(consent.grants[0]),
    }),
    false,
  )
  // Policy contradictions are rejected by Auth, not a second wire decision.
  assert.equal(
    validate("PhoneConsent", { ...consent, grants: [...consent.grants].reverse() }),
    true,
  )
  assert.equal(
    validate("PhoneConsent", { class: "future-auth-class", grants: [consent.grants[0]] }),
    true,
  )
})

test("phone methods name their precise parameter and admission owners", () => {
  const methods = read("manifest.json").methods
  assert.deepEqual(methods["phone.endpoint"], {
    params: "PhoneEndpointParams",
    grant: "server.read",
  })
  assert.deepEqual(methods["phone.authorize"], {
    params: "PhoneAuthorizeParams",
    grant: "credential.manage",
  })
  assert.deepEqual(methods["phone.status"], {
    params: "PhoneStatusParams",
    grant: "credential.manage",
  })
})
