import assert from "node:assert/strict"
import { test } from "node:test"
import { developerIdIdentity } from "./import-certificate.mjs"

const APPLICATION =
  '  1) A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4 "Developer ID Application: Nessa Labs (ABCDE12345)"'
const INSTALLER =
  '  2) 0F1E2D3C4B5A69788796A5B4C3D2E1F009182736 "Developer ID Installer: Nessa Labs (ABCDE12345)"'
const found = (...lines) =>
  `${lines.join("\n")}\n     ${lines.length} valid identities found\n`

test("the Developer ID Application certificate is chosen by hash", () => {
  assert.equal(
    developerIdIdentity(found(APPLICATION)),
    "A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4",
  )
})

/**
 * An installer certificate cannot sign an executable. Taking the first line
 * would pick it whenever it sorts first, and the failure would arrive inside
 * the bundler rather than here.
 */
test("an installer certificate is not mistaken for a signing one", () => {
  assert.equal(
    developerIdIdentity(found(INSTALLER, APPLICATION)),
    "A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4",
  )
  assert.throws(
    () => developerIdIdentity(found(INSTALLER)),
    /not a Developer ID Application/,
  )
})

test("an empty keychain says what is wrong with it", () => {
  assert.throws(
    () => developerIdIdentity("     0 valid identities found\n"),
    /Developer ID/,
  )
})

/** Guessing between two certificates is how a release gets signed by the wrong one. */
test("two signing certificates ask to be told which", () => {
  const second = APPLICATION.replace("1)", "2)").replace("A1B2", "B2A1")
  assert.throws(
    () => developerIdIdentity(found(APPLICATION, second)),
    /APPLE_SIGNING_IDENTITY/,
  )
})
