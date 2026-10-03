import test from "node:test"
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import {
  derivePairingValues,
  pairingArrayOwner,
  pairingValueSchema,
} from "./pairing-values.mjs"

test("owner publication derives every identity/key array and compiled value", () => {
  const values = JSON.parse(
    readFileSync(
      new URL(
        "../../crates/nessa-auth/src/domain/pairing/value_objects/wire-values.json",
        import.meta.url,
      ),
    ),
  )
  const schema = {
    $defs: Object.fromEntries(
      ["InvitationId", "ConsentIntentId", "AttemptId", "DeviceKey"].map((name) => [
        name,
        {
          "x-pairing-byte-array": name,
          minItems: 999,
          maxItems: 999,
        },
      ]),
    ),
  }
  for (const publication of [
    values,
    {
      identityBytes: values.identityBytes + 1,
      deviceKeyBytes: values.deviceKeyBytes + 1,
      manualCodeBytes: values.manualCodeBytes + 1,
    },
  ]) {
    const derived = derivePairingValues(schema, publication)
    assert.deepEqual(pairingValueSchema(publication).schema.$defs.ManualCodeDisplay, {
      type: "string",
      minLength: publication.manualCodeBytes + 1,
      maxLength: publication.manualCodeBytes + 1,
    })
    assert.match(
      derived.rust,
      new RegExp(`MANUAL_CODE_BYTES: usize = ${publication.manualCodeBytes};`),
    )
    for (const [name, node] of Object.entries(derived.schema.$defs)) {
      const width =
        name === "DeviceKey" ? publication.deviceKeyBytes : publication.identityBytes
      assert.equal(node.minItems, width)
      assert.equal(node.maxItems, width)
      assert.deepEqual(node.items, { type: "integer", minimum: 0, maximum: 255 })
      assert.equal(pairingArrayOwner(node), `nessa_auth::domain::pairing::${name}`)
    }
    assert.match(
      derived.rust,
      new RegExp(`IDENTITY_BYTES: usize = ${publication.identityBytes};`),
    )
    assert.match(
      derived.rust,
      new RegExp(`DEVICE_KEY_BYTES: usize = ${publication.deviceKeyBytes};`),
    )
  }
  assert.equal(schema.$defs.InvitationId.minItems, 999)
  assert.throws(() => derivePairingValues({ "x-pairing-byte-array": "toString" }, values))
})

for (const name of ["identityBytes", "deviceKeyBytes", "manualCodeBytes"]) {
  test(`${name} publication requires a positive safe integer`, () => {
    const values = { identityBytes: 16, deviceKeyBytes: 32, manualCodeBytes: 8 }
    const schema = {
      $defs: {
        InvitationId: { "x-pairing-byte-array": "InvitationId" },
        DeviceKey: { "x-pairing-byte-array": "DeviceKey" },
      },
    }
    for (const invalid of [NaN, 1.5, Infinity, -Infinity, 9_007_199_254_740_992, -1, 0]) {
      assert.throws(() => derivePairingValues(schema, { ...values, [name]: invalid }), {
        name: "Error",
        message: `Invalid pairing owner publication: ${name}`,
      })
    }
    for (const valid of [1, Number.MAX_SAFE_INTEGER]) {
      const publication = { ...values, [name]: valid }
      const derived = derivePairingValues(schema, publication)
      assert.equal(derived.schema.$defs.InvitationId.minItems, publication.identityBytes)
      assert.equal(derived.schema.$defs.InvitationId.maxItems, publication.identityBytes)
      assert.equal(derived.schema.$defs.DeviceKey.minItems, publication.deviceKeyBytes)
      assert.equal(derived.schema.$defs.DeviceKey.maxItems, publication.deviceKeyBytes)
      assert.deepEqual(pairingValueSchema(publication).schema.$defs.ManualCodeDisplay, {
        type: "string",
        minLength: publication.manualCodeBytes + 1,
        maxLength: publication.manualCodeBytes + 1,
      })
      for (const [field, constant] of [
        ["identityBytes", "IDENTITY_BYTES"],
        ["deviceKeyBytes", "DEVICE_KEY_BYTES"],
        ["manualCodeBytes", "MANUAL_CODE_BYTES"],
      ]) {
        assert.match(
          derived.rust,
          new RegExp(`${constant}: usize = ${publication[field]};`),
        )
      }
    }
    assert.deepEqual(values, {
      identityBytes: 16,
      deviceKeyBytes: 32,
      manualCodeBytes: 8,
    })
  })
}

test("pairing array owner refuses inherited names at direct lookup", () => {
  for (const name of ["toString", "__proto__", "constructor"]) {
    assert.throws(() => pairingArrayOwner({ "x-pairing-byte-array": name }), {
      name: "Error",
      message: `Unknown pairing value owner: ${name}`,
    })
  }
  for (const name of ["InvitationId", "ConsentIntentId", "AttemptId", "DeviceKey"]) {
    assert.equal(
      pairingArrayOwner({ "x-pairing-byte-array": name }),
      `nessa_auth::domain::pairing::${name}`,
    )
  }
  assert.equal(pairingArrayOwner({}), undefined)
})
