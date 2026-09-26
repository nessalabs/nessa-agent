import assert from "node:assert/strict"
import {
  mkdirSync,
  mkdtempSync,
  rmSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import {
  REQUIRED_DEB_PACKAGES,
  buildSelection,
  builtPackage,
  packageBuiltIn,
  dependencyClauses,
  missingDependencies,
} from "./verify-linux-bundle.mjs"

test("a Depends field is read as clauses of alternatives, versions dropped", () => {
  assert.deepEqual(
    dependencyClauses(
      "libwebkit2gtk-4.1-0 (>= 2.36), libgtk-3-0, libayatana-appindicator3-1 | libappindicator3-1, libc6:amd64",
    ),
    [
      ["libwebkit2gtk-4.1-0"],
      ["libgtk-3-0"],
      ["libayatana-appindicator3-1", "libappindicator3-1"],
      ["libc6"],
    ],
  )
  assert.deepEqual(dependencyClauses("\n"), [])
})

test("the .deb must declare WebKitGTK and the tray's indicator library", () => {
  assert.deepEqual(REQUIRED_DEB_PACKAGES, [
    "libwebkit2gtk-4.1-0",
    "libayatana-appindicator3-1",
  ])
})

test("a package that leaves out a required library is named, alternatives count", () => {
  const required = REQUIRED_DEB_PACKAGES
  assert.deepEqual(missingDependencies("libwebkit2gtk-4.1-0, libgtk-3-0", required), [
    "libayatana-appindicator3-1",
  ])
  assert.deepEqual(
    missingDependencies(
      "libwebkit2gtk-4.1-0, libappindicator3-1 | libayatana-appindicator3-1",
      required,
    ),
    [],
  )
  // Versions and architecture qualifiers do not change which package is named.
  assert.deepEqual(
    missingDependencies(
      "libwebkit2gtk-4.1-0 (>= 2.36), libayatana-appindicator3-1:amd64",
      required,
    ),
    [],
  )
  // A longer name that starts with the required one is a different package.
  assert.deepEqual(
    missingDependencies("libwebkit2gtk-4.1-0-dbg", ["libwebkit2gtk-4.1-0"]),
    ["libwebkit2gtk-4.1-0"],
  )
})

test("the package checked is the one this build wrote, whatever its version", () => {
  const started = 1_790_000_000_500
  const pattern = "Nessa_*_amd64.deb"
  // An older app built by merging another version, beside the shipped one an
  // earlier build left: the newer write is this build's.
  assert.equal(
    builtPackage(
      [
        { name: "Nessa_0.1.0_amd64.deb", modified: started - 60_000 },
        { name: "Nessa_0.0.1_amd64.deb", modified: started + 90_000 },
      ],
      started,
      pattern,
    ),
    "Nessa_0.0.1_amd64.deb",
  )
  // Written in the second the build began: file times may be whole seconds.
  assert.equal(
    builtPackage(
      [{ name: "Nessa_0.1.0_amd64.deb", modified: 1_790_000_000_000 }],
      started,
      pattern,
    ),
    "Nessa_0.1.0_amd64.deb",
  )
  assert.throws(
    () =>
      builtPackage(
        [{ name: "Nessa_0.1.0_amd64.deb", modified: started - 60_000 }],
        started,
        pattern,
      ),
    /Nessa_\*_amd64\.deb written by this build, found none/,
  )
  assert.throws(
    () =>
      builtPackage(
        [
          { name: "Nessa_0.1.0_amd64.deb", modified: started + 1 },
          { name: "Nessa_0.0.1_amd64.deb", modified: started + 2 },
        ],
        started,
        pattern,
      ),
    /found Nessa_0.1.0_amd64.deb, Nessa_0.0.1_amd64.deb/,
  )
  // A start that is not a time says so, rather than finding no package.
  assert.throws(
    () =>
      builtPackage([{ name: "Nessa_0.1.0_amd64.deb", modified: started }], NaN, pattern),
    /not a time/,
  )
})

test("the package is found in the bundle's own directory for the pattern", () => {
  const bundle = mkdtempSync(join(tmpdir(), "nessa-built-"))
  try {
    mkdirSync(join(bundle, "deb/Nessa_0.1.0_amd64"), { recursive: true })
    const old = join(bundle, "deb/Nessa_0.1.0_amd64.deb")
    writeFileSync(old, "old")
    utimesSync(old, new Date(1_000_000_000_000), new Date(1_000_000_000_000))
    writeFileSync(join(bundle, "deb/Nessa_0.0.1_amd64.deb"), "new")
    writeFileSync(join(bundle, "deb/Nessa_0.0.1_amd64.deb.sig"), "sig")
    // Not a candidate, so never examined: a dangling link cannot fail the check.
    symlinkSync("absent", join(bundle, "deb/unrelated"))
    assert.equal(
      packageBuiltIn(bundle, "deb/Nessa_*_amd64.deb", 1_500_000_000_000),
      join(bundle, "deb/Nessa_0.0.1_amd64.deb"),
    )
  } finally {
    rmSync(bundle, { recursive: true, force: true })
  }
})

test("the verifier checks only what a build told it it made", () => {
  assert.deepEqual(
    buildSelection({ NESSA_BUILD_BUNDLES: "deb", NESSA_BUILD_STARTED: "1790000000000" }),
    { started: 1_790_000_000_000 },
  )
  // Run on its own, it refuses rather than guessing either.
  for (const environment of [
    {},
    { NESSA_BUILD_BUNDLES: "deb" },
    { NESSA_BUILD_STARTED: "1790000000000" },
  ])
    assert.throws(() => buildSelection(environment), /Run through `pnpm app:build`/)
  // It verifies the .deb alone, and refuses to be told otherwise.
  for (const bundles of ["deb,rpm", "appimage", "all"])
    assert.throws(
      () => buildSelection({ NESSA_BUILD_BUNDLES: bundles, NESSA_BUILD_STARTED: "1" }),
      /verifies the \.deb alone/,
    )
})
