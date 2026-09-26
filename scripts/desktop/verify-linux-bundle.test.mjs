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
  builtPackage,
  packageBuiltIn,
  uncheckedBundles,
  carriesIndicatorLibrary,
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

test("an AppImage tree without the tray's indicator library is refused", () => {
  const root = mkdtempSync(join(tmpdir(), "nessa-indicator-"))
  try {
    mkdirSync(join(root, "usr/lib"), { recursive: true })
    writeFileSync(join(root, "usr/lib/libgtk-3.so.0"), "")
    assert.equal(carriesIndicatorLibrary(root), false)
    // An empty file and a dangling link load nothing.
    writeFileSync(join(root, "usr/lib/libayatana-appindicator3.so.1"), "")
    assert.equal(carriesIndicatorLibrary(root), false)
    rmSync(join(root, "usr/lib/libayatana-appindicator3.so.1"))
    symlinkSync("absent.so", join(root, "usr/lib/libayatana-appindicator3.so.1"))
    assert.equal(carriesIndicatorLibrary(root), false)
    rmSync(join(root, "usr/lib/libayatana-appindicator3.so.1"))
    writeFileSync(join(root, "usr/lib/libayatana-appindicator3.so.1.0.0"), "\x7fELF")
    symlinkSync(
      "libayatana-appindicator3.so.1.0.0",
      join(root, "usr/lib/libayatana-appindicator3.so.1"),
    )
    assert.equal(carriesIndicatorLibrary(root), true)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
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

test("a build is refused when it made a bundle no Linux check opens", () => {
  assert.deepEqual(uncheckedBundles("deb", "all"), [])
  assert.deepEqual(uncheckedBundles("deb,appimage", "all"), [])
  assert.deepEqual(uncheckedBundles("deb,rpm", "all"), ["rpm"])
  // With nothing named, the config's "all" includes rpm.
  assert.deepEqual(uncheckedBundles(undefined, "all"), ["all"])
  assert.deepEqual(uncheckedBundles(undefined, ["deb"]), [])
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
