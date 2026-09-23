/**
 * Serve a signed release to a local build, so the updater can be run for real
 * before anything is published.
 *
 * The updater has one failure mode nobody finds by reading code: the shipped
 * public key and the signing key are not two halves of the same pair, so every
 * build rejects every update forever. `src-tauri/tests/updater_key_pairing.rs`
 * settles that question automatically. This settles the rest of the path —
 * manifest shape, target key, download, verification, install, restart — by
 * running it, against a server on this machine, with nothing published.
 *
 *   built artifact ──sign──▶ latest.json ──┐
 *                                          ├──▶ http://localhost:<port>
 *   built artifact ──copy────────────────▶ ┘            ▲
 *                                                       │ endpoints
 *                     older build ── tray ── check ─────┘
 *
 * ## Running it
 *
 *   1. Build normally, so there is an artifact to serve. This is the *new*
 *      version, the one an update installs:
 *
 *        pnpm app:build
 *
 *   2. Start this, pointing at what that produced (it finds the usual place on
 *      its own if you leave `--artifact` off):
 *
 *        TAURI_SIGNING_PRIVATE_KEY_PATH=~/.nessa-signing/updater.key \
 *        TAURI_SIGNING_PRIVATE_KEY_PASSWORD= \
 *          node scripts/desktop/updater-harness.mjs
 *
 *      It signs the artifact, copies it somewhere the next build will not
 *      overwrite, writes `latest.json` beside it, serves both, and prints the
 *      build command for step 3.
 *
 *   3. In another terminal, run the command it printed. That builds the *older*
 *      app: the same code with a lower version number and the updater endpoint
 *      merged in at build time. Install and launch it.
 *
 * The endpoint only ever moves through that `--config` merge. The shipped
 * `tauri.conf.json` keeps the GitHub endpoint and gains no switch, because a
 * setting that redirects the updater is a setting an attacker can redirect it
 * with.
 *
 * ## What success looks like
 *
 * The older app starts and looks completely ordinary — the check is quiet by
 * design, and nothing takes the screen. Open the panel: within a second or two
 * of launch a notice sits above the composer reading "Update available" with
 * the served version under it, and two icon controls. Take the install one. The
 * notice goes, an update tab opens on the download, and the app downloads the
 * artifact, verifies the signature, installs over itself, and restarts; the app
 * that comes back is the new version. This terminal logs exactly two requests:
 * one for `/latest.json` at launch, one for the artifact on the install.
 *
 * ## What each failure means
 *
 * The three things being tested fail in three distinguishable ways.
 *
 * - **Endpoint unreachable — no notice, and no requests logged here.** The
 *   check never got an answer. The build did not take the `--config` merge, or
 *   it is pointed at a different port, or this server is not running. The app's
 *   stderr says `could not check for an update:` and names the reason.
 *
 * - **Manifest unreadable or inapplicable — no notice, but `/latest.json`
 *   *was* requested.** The manifest arrived and produced no update: a version
 *   that is not newer than the running build's (it must be built at the lower
 *   version this prints), a `pub_date` that is not RFC 3339, or a `platforms`
 *   key that is not this machine's target triple. Signing is not involved; the
 *   stderr line distinguishes a parse failure from a target that was not found.
 *
 * - **Signature rejected — the notice appears, the update tab says the
 *   download did not finish, the app's stderr says `could not install the
 *   update: signature ...`, and the artifact *was* requested.**
 *   This is the unrecoverable failure, caught before release: the public key in
 *   `tauri.conf.json` does not verify what the private key signed, or the bytes
 *   served are not the bytes that were signed. Run the key-pair gate:
 *   `cargo test -p nessa-app --test updater_key_pairing --no-default-features`.
 *
 *   If the tab reports a refusal and the artifact was *not* requested, the
 *   download never started and the `url` in the manifest is wrong — not the
 *   signature.
 *
 * ## Check-only mode, for a dev run
 *
 * A release build is slow and needs the whole bundled runtime, which is a lot
 * to pay to watch a check happen. `--check-only` serves a well-formed manifest
 * and no artifact:
 *
 *   node scripts/desktop/updater-harness.mjs --check-only
 *
 * then, in another terminal, the documented `--config` merge on `dev`:
 *
 *   pnpm tauri dev --config '{"plugins":{"updater":{"endpoints":["http://127.0.0.1:7430/latest.json"]}}}'
 *
 * The plugin does the whole of its check for real — fetches over HTTP, parses
 * the manifest, reads `pub_date`, looks up this machine's target key, compares
 * versions — and the notice appears in the panel. It stops exactly there. The
 * announced URL 404s, so the update tab ends in a refusal and a retry;
 * signature verification is never
 * reached, and the manifest's `signature` field is an honest sentence saying so
 * rather than anything that could be mistaken for a signature. Install and
 * restart are untested in this mode. It is not an end-to-end pass and the
 * banner it prints says so.
 *
 * Neither this nor the full harness is the fastest loop. Nothing here needs a
 * server to exercise the *decision*, the notice, and the install: a debug
 * build reads `NESSA_FAKE_UPDATE=9.9.9` and answers from it with no network at
 * all (`src-tauri/src/updater.rs`). That path installs nothing and says so.
 *
 * ## What this cannot tell you
 *
 * Only that the plugin accepts what *this* machine signs and serves. It does
 * not prove the release workflow signs with the same key — that is the
 * key-pair gate, which `.github/workflows/release.yml` runs with the workflow's
 * own secret and `NESSA_REQUIRE_UPDATER_KEY_PAIRING=1` before it builds
 * anything — and it does not exercise GitHub's release hosting, its redirects,
 * or its TLS.
 */

import { spawnSync } from "node:child_process"
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs"
import { createServer } from "node:http"
import { basename, resolve } from "node:path"
import { cargoTargetDirectory } from "../cargo-target.mjs"
import { option } from "./cli.mjs"
import { requestedPath } from "./request-target.mjs"
import {
  CHECK_ONLY_ARTIFACT,
  checkOnlyManifest,
  defaultArtifacts,
  releaseManifest,
  updaterTarget,
} from "./updater-manifest.mjs"

/** Sign the artifact with the release private key, the way a release does. */
function sign(root, artifact) {
  const path = process.env.TAURI_SIGNING_PRIVATE_KEY_PATH
  const key = process.env.TAURI_SIGNING_PRIVATE_KEY
  if (!path && !key)
    throw new Error(
      "No release private key. Set TAURI_SIGNING_PRIVATE_KEY_PATH (or " +
        "TAURI_SIGNING_PRIVATE_KEY) to the key releases are signed with, so this " +
        "artifact is verified against the same key a real one would be.",
    )

  const pnpm = process.platform === "win32" ? "pnpm.cmd" : "pnpm"
  const signed = spawnSync(
    pnpm,
    [
      "exec",
      "tauri",
      "signer",
      "sign",
      ...(path ? ["--private-key-path", path] : ["--private-key", key]),
      "--password",
      process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? "",
      artifact,
    ],
    { cwd: root, encoding: "utf8" },
  )
  if (signed.error) throw signed.error
  if (signed.status !== 0)
    throw new Error(`tauri signer sign failed:\n${signed.stderr || signed.stdout}`)
  // Tauri writes the signature beside the file: base64 of the whole minisign
  // block, which is exactly what the manifest's `signature` field holds.
  return readFileSync(`${artifact}.sig`, "utf8").trim()
}

const root = resolve(import.meta.dirname, "../..")
const args = process.argv.slice(2)
const config = JSON.parse(
  readFileSync(resolve(root, "src-tauri/tauri.conf.json"), "utf8"),
)

// Serve a manifest and nothing else: no artifact, no signing key, no release
// build. The plugin's check runs for real against it; its install cannot.
const checkOnly = args.includes("--check-only")

const port = Number(option(args, "port", "7430"))
const origin = `http://127.0.0.1:${port}`
// A check-only run is compared against whatever `pnpm tauri dev` reports, which
// is the shipped version, so it announces something plainly above it instead of
// asking for a lowered build.
const version = option(args, "version", checkOnly ? "99.0.0" : config.version)
const notes = option(args, "notes", `Local harness build of ${config.productName}.`)
const target = option(args, "target", updaterTarget(process.platform, process.arch))
// Low enough that the served manifest is unambiguously newer, and obviously not
// a real version wherever it shows up.
const buildVersion = option(args, "build-version", "0.0.1")

const named = option(args, "artifact", undefined)
const published = new Date().toISOString()
const served = resolve(root, "target/updater-harness")
mkdirSync(served, { recursive: true })

/** In check-only mode: a manifest, an announced name, and no bytes anywhere. */
function withoutAnArtifact() {
  return {
    name: CHECK_ONLY_ARTIFACT,
    copy: undefined,
    url: `${origin}/${CHECK_ONLY_ARTIFACT}`,
    manifest: checkOnlyManifest({ version, notes, target, origin, published }),
  }
}

/** The full path: find what a release build produced, sign it, serve it. */
function withTheBuiltArtifact() {
  const candidates = (
    named
      ? [named]
      : defaultArtifacts(process.platform, config.version, cargoTargetDirectory(root))
  ).map((candidate) => resolve(root, candidate))
  const artifact = candidates.find((candidate) => existsSync(candidate))
  if (!artifact) {
    console.error(
      `No updater artifact to serve. Looked for:\n` +
        `${candidates.map((candidate) => `  ${candidate}`).join("\n")}\n\n` +
        `Build one first — this deliberately does not, because a release build is slow\n` +
        `and needs the whole bundled runtime:\n\n` +
        `  pnpm app:build\n\n` +
        `then re-run, or pass --artifact <path> if yours is somewhere else.\n\n` +
        `To exercise only the *check* — real HTTP, real manifest parse, real version\n` +
        `comparison, no download and no signature verification — no artifact is needed:\n\n` +
        `  node scripts/desktop/updater-harness.mjs --check-only`,
    )
    process.exit(1)
  }

  // Copied out of the bundle directory before anything is signed: the build in
  // step 3 writes to that same directory and would replace the artifact under
  // the signature about to be made, leaving a manifest describing bytes that
  // are no longer there — which fails as a rejected signature and looks like a
  // key fault.
  const name = basename(artifact)
  const copy = resolve(served, name)
  copyFileSync(artifact, copy)
  const url = `${origin}/${encodeURIComponent(name)}`

  return {
    name,
    copy,
    url,
    manifest: releaseManifest({
      version,
      notes,
      platforms: { [target]: { signature: sign(root, copy), url } },
      published,
    }),
  }
}

const { name, copy, url, manifest } = checkOnly
  ? withoutAnArtifact()
  : withTheBuiltArtifact()
const manifestBody = Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`)
writeFileSync(resolve(served, "latest.json"), manifestBody)

// The harness serves over http on the loopback interface, and the plugin
// refuses a non-https endpoint in a release build — `validate_endpoints`
// returns `InsecureTransportProtocol` there, where a debug build only prints a
// warning. So the generated configuration says it means it. It is generated,
// never written to `tauri.conf.json`: the shipped product stays https-only and
// has no such permission in it, which is the same reason the endpoint itself
// is a `--config` merge rather than an edit.
const localEndpoint = {
  endpoints: [`${origin}/latest.json`],
  dangerousInsecureTransportProtocol: true,
}
const endpointConfig = JSON.stringify({
  version: buildVersion,
  plugins: { updater: localEndpoint },
})
const devConfig = JSON.stringify({
  plugins: { updater: localEndpoint },
})

const server = createServer((request, response) => {
  // Two paths, matched exactly. A harness that serves a directory is a file
  // server pointed at a build tree, which is more than this needs to be.
  //
  // Read through `requestedPath`, which is the only thing here allowed to look
  // at a request target: both halves of interpreting one throw, and a throw in
  // this callback ends the harness rather than the request.
  const asked = requestedPath(request.url, origin)
  const body =
    asked === "/latest.json"
      ? { bytes: manifestBody, type: "application/json" }
      : copy && asked === `/${name}`
        ? { bytes: readFileSync(copy), type: "application/octet-stream" }
        : undefined
  console.log(`  ${request.method} ${asked ?? request.url} -> ${body ? 200 : 404}`)
  if (!body) {
    // The one 404 worth explaining: a click on the offered item in check-only
    // mode. It is the mode working as described, not a fault to chase.
    if (checkOnly && asked === `/${name}`)
      console.log(
        `  ^ the announced artifact does not exist: --check-only serves no bytes, so the\n` +
          `    download fails here and signature verification is never reached. Run the full\n` +
          `    harness against a real 'pnpm app:build' artifact to test install and restart.`,
      )
    response.writeHead(404).end()
    return
  }
  response.writeHead(200, {
    "content-type": body.type,
    "content-length": body.bytes.length,
  })
  response.end(body.bytes)
})

server.listen(port, "127.0.0.1", () => {
  const banner = checkOnly
    ? [
        ``,
        `CHECK ONLY — announcing version ${version} for ${target}, with no artifact behind it.`,
        `  manifest  ${origin}/latest.json`,
        `  artifact  ${url}  (announced; this server answers 404 for it)`,
        ``,
        `Point a dev run at it, in another terminal — no release build, no signing key:`,
        ``,
        `  pnpm tauri dev --config '${devConfig}'`,
        ``,
        `The running app reports itself as ${config.version}, so ${version} reads as newer and the`,
        `panel's notice reads "Update available", with ${version} under it.`,
        ``,
        `What this mode tests, for real, through the actual tauri-plugin-updater:`,
        `  - the endpoint is fetched over HTTP by the plugin, not by anything in this repo`,
        `  - the manifest is parsed by the plugin, including its RFC 3339 pub_date`,
        `  - the ${target} key is looked up and the versions are compared by the plugin`,
        `  - the announcement reaches the panel through the app's own check`,
        ``,
        `What this mode does NOT test, at all:`,
        `  - the download: the announced URL 404s here, on purpose, so the tab`,
        `    ends in its refused state — which is itself worth seeing once`,
        `  - signature verification: never reached, and the manifest's signature field is a`,
        `    sentence, not a signature`,
        `  - install, restart, and coming back up on the new version`,
        `A clean run here is not an end-to-end pass. For those four, build a real artifact`,
        `and run this without --check-only.`,
        ``,
        `Requests arrive below; Ctrl-C when you are done.`,
        ``,
      ]
    : [
        ``,
        `Serving ${name} (${(statSync(copy).size / 1024 / 1024).toFixed(1)} MiB) as version ${version} for ${target}`,
        `  manifest  ${origin}/latest.json`,
        `  artifact  ${url}`,
        ``,
        `Now build the older app that will find it, in another terminal:`,
        ``,
        `  pnpm app:build --config '${endpointConfig}'`,
        ``,
        `Install and launch what that produces. It reports itself as ${buildVersion}, so the`,
        `manifest above reads as newer and the panel's notice offers ${version}.`,
        `Requests arrive below; Ctrl-C when you are done.`,
        ``,
      ]
  console.log(banner.join("\n"))
})
