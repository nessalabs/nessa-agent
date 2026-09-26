import assert from "node:assert/strict"
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import {
  REQUIRED_DEB_PACKAGES,
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
