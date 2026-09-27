import assert from "node:assert/strict"
import test from "node:test"
import { validateExternalRustTypes } from "./rust-types.mjs"

test("external Rust imports require qualified paths", () => {
  for (const path of [
    "ResourceDto",
    "",
    "::ResourceDto",
    "module::",
    42,
    "module::Type<T>",
  ]) {
    assert.throws(
      () => validateExternalRustTypes({ Input: { "x-rust-type": path } }),
      /qualified Rust type path/,
    )
  }
})

test("external Rust imports cannot collide with each other or generated names", () => {
  assert.throws(
    () =>
      validateExternalRustTypes({
        First: { "x-rust-type": "first::ResourceDto" },
        Second: { "x-rust-type": "second::ResourceDto" },
      }),
    /Rust type name ResourceDto conflicts/,
  )
  assert.throws(
    () =>
      validateExternalRustTypes({
        ResourceDto: {},
        Input: { "x-rust-type": "external::ResourceDto" },
      }),
    /generated type ResourceDto/,
  )
  assert.throws(
    () => validateExternalRustTypes({ Input: { "x-rust-type": "external::Option" } }),
    /reserved type Option/,
  )
})

test("identical external paths can be referenced by multiple definitions", () => {
  assert.doesNotThrow(() =>
    validateExternalRustTypes({
      First: { "x-rust-type": "external::ResourceDto" },
      Second: { "x-rust-type": "external::ResourceDto" },
      Third: { "x-rust-type": "external::OtherDto" },
    }),
  )
})

test("external Rust imports cannot shadow generated primitive types", () => {
  for (const name of ["u64", "u16", "bool"]) {
    assert.throws(
      () => validateExternalRustTypes({ Input: { "x-rust-type": `external::${name}` } }),
      new RegExp(`reserved type ${name}`),
    )
  }
})
