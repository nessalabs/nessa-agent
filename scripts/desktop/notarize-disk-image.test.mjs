import assert from "node:assert/strict"
import { test } from "node:test"
import { diskImageToNotarize, submissionArguments } from "./notarize-disk-image.mjs"

const KEY = {
  keyPath: "/work/.apple-api-key.p8",
  keyId: "ABCDE12345",
  issuer: "1234-5678",
}

/**
 * `--wait` is the whole reason this is safe to follow with a staple: without
 * it the submission returns an id and the ticket does not exist yet.
 */
test("a submission waits for the ticket it is about to staple", () => {
  const args = submissionArguments("/bundle/dmg/Nessa_0.1.0_aarch64.dmg", KEY)
  assert.deepEqual(args.slice(0, 2), ["submit", "/bundle/dmg/Nessa_0.1.0_aarch64.dmg"])
  assert.ok(args.includes("--wait"))
  assert.deepEqual(
    [
      args[args.indexOf("--key") + 1],
      args[args.indexOf("--key-id") + 1],
      args[args.indexOf("--issuer") + 1],
    ],
    [KEY.keyPath, KEY.keyId, KEY.issuer],
  )
})

/** Half a credential set fails here, naming the half that is missing. */
test("an incomplete key is refused by name", () => {
  assert.throws(() => submissionArguments("/x.dmg", { ...KEY, issuer: "  " }), /issuer/)
  assert.throws(() => submissionArguments("/x.dmg", {}), /keyPath.*keyId.*issuer/)
})

/**
 * No key means the app was not notarized either, so there is no ticket for the
 * disk image to be missing. Nothing to do is not a failure.
 */
test("a build with no App Store Connect key has nothing to staple", () => {
  assert.equal(
    diskImageToNotarize({
      environment: { NESSA_BUILD_BUNDLES: "app,dmg" },
      root: process.cwd(),
      architecture: "arm64",
    }),
    undefined,
  )
})

/** A build that produced no disk image has none to notarize. */
test("a build of only the app has no disk image", () => {
  assert.equal(
    diskImageToNotarize({
      environment: { APPLE_API_KEY: "ABCDE12345", NESSA_BUILD_BUNDLES: "app" },
      root: process.cwd(),
      architecture: "arm64",
    }),
    undefined,
  )
})

/** The path is the one the bundler wrote, per architecture. */
test("the disk image is found where the bundler put it", () => {
  const image = diskImageToNotarize({
    environment: {
      APPLE_API_KEY: "ABCDE12345",
      NESSA_BUILD_BUNDLES: "app,dmg",
      NESSA_BUILD_TARGET: "x86_64-apple-darwin",
    },
    root: process.cwd(),
    architecture: "arm64",
  })
  assert.match(
    image,
    /x86_64-apple-darwin\/release\/bundle\/dmg\/Nessa_\d+\.\d+\.\d+_x64\.dmg$/,
  )
})
