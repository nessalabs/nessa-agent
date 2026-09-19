import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"
import {
  agreementReport,
  cargoVersion,
  declaredVersions,
  jsonVersion,
  taggedVersion,
  versionAgreement,
} from "./release-version.mjs"

test("each file's version is read from the place that file declares it", () => {
  assert.equal(jsonVersion(`{"name":"nessa-app","version":"0.1.0"}`), "0.1.0")
  assert.equal(jsonVersion(`{"name":"nessa-app"}`), undefined)
  assert.equal(jsonVersion(`{"productName":"Nessa","version":"0.2.1"}`), "0.2.1")
  assert.equal(
    cargoVersion('[package]\nname = "nessa-app"\nversion = "0.1.0" # shipped\n'),
    "0.1.0",
  )
})

test("only the Cargo package's own version counts", () => {
  // A `version` under `[dependencies]` is some crate's requirement, not this
  // package's number, and reading one as the other would pass a release that
  // ships something else entirely.
  const manifest = [
    "[workspace.package]",
    'version = "9.9.9"',
    "",
    "[package]",
    'name = "nessa-app"',
    'version = "0.1.0"',
    "",
    "[dependencies]",
    'tauri = { version = "2" }',
  ].join("\n")
  assert.equal(cargoVersion(manifest), "0.1.0")
})

test("a version this cannot read is absent, never assumed", () => {
  // Workspace inheritance is legal Cargo and this deliberately does not follow
  // it. The gate then fails rather than passing on a number it never saw: an
  // unverified version must not read as an agreeing one.
  assert.equal(
    cargoVersion('[package]\nname = "nessa-app"\nversion.workspace = true\n'),
    undefined,
  )
  assert.equal(cargoVersion('[dependencies]\nserde = { version = "1" }\n'), undefined)
})

test("a release tag is a v and a semantic version, or it is not one", () => {
  assert.equal(taggedVersion("v0.1.0"), "0.1.0")
  assert.equal(taggedVersion("refs/tags/v0.1.0"), "0.1.0")
  assert.equal(taggedVersion("v1.2.3-rc.1"), "1.2.3-rc.1")
  for (const tag of ["0.1.0", "v0.1", "version-0.1.0", "main", "", undefined])
    assert.equal(taggedVersion(tag), undefined, `${tag} is not a release tag`)
})

test("agreement is the tag and every declared version saying one thing", () => {
  const sources = [
    { name: "package.json", version: "0.1.0" },
    { name: "src-tauri/Cargo.toml", version: "0.1.0" },
    { name: "src-tauri/tauri.conf.json", version: "0.1.0" },
  ]
  assert.deepEqual(versionAgreement({ tag: "v0.1.0", sources }), {
    kind: "Agreed",
    version: "0.1.0",
  })
})

test("a disagreeing file names itself, and the release stops", () => {
  // The expensive one: the tag and the bundle report different versions, the
  // manifest never matches what is installed, and nothing anywhere errors.
  const sources = [
    { name: "package.json", version: "0.1.0" },
    { name: "src-tauri/Cargo.toml", version: "0.1.0" },
    { name: "src-tauri/tauri.conf.json", version: "0.0.9" },
  ]
  const agreement = versionAgreement({ tag: "v0.1.0", sources })
  assert.equal(agreement.kind, "Disagreed")
  assert.deepEqual(
    agreement.disagreeing.map((source) => source.name),
    ["src-tauri/tauri.conf.json"],
  )
  const report = agreementReport(agreement)
  assert.match(report, /src-tauri\/tauri\.conf\.json: 0\.0\.9/)
  assert.match(report, /Nothing was built and nothing was published/)
})

test("an undeclared version is a disagreement, not a pass", () => {
  const agreement = versionAgreement({
    tag: "v0.1.0",
    sources: [{ name: "src-tauri/Cargo.toml", version: undefined }],
  })
  assert.equal(agreement.kind, "Disagreed")
  assert.match(agreementReport(agreement), /\(no version declared\)/)
})

test("a ref that is not a release tag is refused before anything is read", () => {
  const agreement = versionAgreement({ tag: "refs/heads/main", sources: [] })
  assert.equal(agreement.kind, "Unnamed")
  assert.match(agreementReport(agreement), /is not a release tag/)
})

test("the three versions this repository carries today agree with each other", () => {
  // Not a tag check — there is no tag here. It is the standing invariant the
  // release gate depends on: these three files are independent and drift.
  const declared = declaredVersions(process.cwd())
  assert.deepEqual(
    declared.map((source) => source.name),
    ["package.json", "src-tauri/Cargo.toml", "src-tauri/tauri.conf.json"],
  )
  const shipped = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8")).version
  for (const source of declared) assert.equal(source.version, shipped, source.name)
})
