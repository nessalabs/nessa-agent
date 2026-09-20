import assert from "node:assert/strict"
import { test } from "node:test"
import {
  RUNTIME_EXECUTABLES,
  runtimeEntitlements,
  signingArguments,
  signingIdentity,
  signingProblems,
} from "./runtime-signing.mjs"

const IDENTITY = "Developer ID Application: Nessa Labs (ABCDE12345)"

/**
 * A build with no credentials signs exactly as it always did.
 *
 * This is a developer's own `pnpm app:build`, and it has no identity to sign
 * with. An ad-hoc signature is what makes the binary runnable on the machine
 * that produced it; asking for a timestamp or the hardened runtime here would
 * fail the build for a bundle nobody is distributing.
 */
test("no identity signs ad hoc", () => {
  assert.deepEqual(signingArguments("/runtime/nessa"), [
    "--force",
    "--sign",
    "-",
    "/runtime/nessa",
  ])
})

/**
 * The three things the notary service asked for, together.
 *
 * Apple rejected v0.1.0 with one error per missing piece per binary: not a
 * Developer ID signature, no secure timestamp, no hardened runtime. Any two of
 * these without the third is still a rejected release.
 */
test("an identity signs with a timestamp and the hardened runtime", () => {
  assert.deepEqual(signingArguments("/runtime/nessa", { identity: IDENTITY }), [
    "--force",
    "--sign",
    IDENTITY,
    "--timestamp",
    "--options",
    "runtime",
    "/runtime/nessa",
  ])
})

test("entitlements are passed when the binary has any", () => {
  const args = signingArguments("/runtime/node", {
    identity: IDENTITY,
    entitlements: "/src-tauri/Entitlements.node.plist",
  })
  assert.deepEqual(args.slice(-3), [
    "--entitlements",
    "/src-tauri/Entitlements.node.plist",
    "/runtime/node",
  ])
})

/** Only Node compiles code at runtime, so only Node is owed an exception. */
test("node is the only executable with entitlements", () => {
  assert.equal(runtimeEntitlements("node"), "Entitlements.node.plist")
  for (const name of RUNTIME_EXECUTABLES.filter((name) => name !== "node"))
    assert.equal(runtimeEntitlements(name), undefined, name)
})

/** An ad-hoc signature takes no entitlements, whatever the binary is. */
test("entitlements are ignored without an identity", () => {
  const args = signingArguments("/runtime/node", {
    entitlements: "/src-tauri/Entitlements.node.plist",
  })
  assert.ok(!args.includes("--entitlements"), args.join(" "))
})

const SIGNED = `Executable=/Nessa.app/Contents/Resources/runtime/node
Identifier=node
Format=Mach-O thin (arm64)
CodeDirectory v=20500 size=282726 flags=0x10000(runtime) hashes=8829+2 location=embedded
Signature size=9045
Authority=Developer ID Application: Nessa Labs (ABCDE12345)
Authority=Developer ID Certification Authority
Authority=Apple Root CA
Timestamp=19 Sep 2026 at 23:30:03
`
const ADHOC = `Executable=/Nessa.app/Contents/Resources/runtime/node
Identifier=node-555549441feaf41ce9af39ee86e4c387edc5ed44
Format=Mach-O thin (arm64)
CodeDirectory v=20400 size=282726 flags=0x2(adhoc) hashes=8829+2 location=embedded
Signature=adhoc
`

test("a Developer ID signature with a hardened runtime has nothing wrong with it", () => {
  assert.deepEqual(signingProblems("node", SIGNED), [])
})

/**
 * The exact signature Apple rejected v0.1.0 for, and all three reasons it gave.
 * `codesign --verify` passes on this, which is why it is checked separately.
 */
test("an ad-hoc signature is reported the way the notary log reported it", () => {
  const problems = signingProblems("node", ADHOC)
  assert.equal(problems.length, 3)
  assert.match(problems.join("\n"), /Developer ID Application/)
  assert.match(problems.join("\n"), /hardened runtime/)
  assert.match(problems.join("\n"), /secure timestamp/)
})

/** Signed by the right certificate is not the same as signed the right way. */
test("a Developer ID signature without the hardened runtime is still wrong", () => {
  const problems = signingProblems("nessa", SIGNED.replace("0x10000(runtime)", "0x0"))
  assert.deepEqual(problems, ["nessa does not have the hardened runtime enabled"])
})

/**
 * Every way of saying "no certificate", including the one that looks like a
 * certificate. `APPLE_SIGNING_IDENTITY=-` is Tauri's documented way to ask for
 * an ad-hoc signature; taken literally it signs with `--options runtime` and
 * then fails the check that reads a Developer ID authority back.
 */
test("unset, empty and - all mean an ad-hoc signature", () => {
  for (const value of [undefined, "", "   ", "-", " - "])
    assert.equal(signingIdentity(value), undefined, JSON.stringify(value))
  assert.equal(signingIdentity(` ${IDENTITY} `), IDENTITY)
  assert.equal(
    signingIdentity("A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4"),
    "A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4",
  )
})

test("an explicit - signs ad-hoc rather than asking for a hardened runtime", () => {
  assert.deepEqual(signingArguments("/runtime/node", { identity: "-" }), [
    "--force",
    "--sign",
    "-",
    "/runtime/node",
  ])
  assert.deepEqual(
    signingArguments("/runtime/node", { identity: "-" }),
    signingArguments("/runtime/node"),
    "an explicit ad-hoc identity takes a different path from an absent one",
  )
})

/** A name with spaces around it is the same name. */
test("an identity is trimmed before it is signed with", () => {
  assert.deepEqual(
    signingArguments("/runtime/nessa", { identity: ` ${IDENTITY} ` }).slice(0, 3),
    ["--force", "--sign", IDENTITY],
  )
})
