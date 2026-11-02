import { test } from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdtemp, mkdir, readFile, writeFile, cp, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const here = dirname(fileURLToPath(import.meta.url))
const root = resolve(here, "../..")
const script = join(here, "check.mjs")
function run(path, args = [], env = {}) {
  const child = spawnSync(process.execPath, [path, ...args], {
    env: { ...process.env, ...env },
    encoding: "utf8",
    timeout: 120_000,
  })
  assert.ifError(child.error)
  assert.equal(child.stderr, "")
  return { code: child.status, result: JSON.parse(child.stdout) }
}

test("help, invalid combinations and absent renderer report honest statuses", () => {
  assert.equal(run(script, ["--help"]).code, 0)
  for (const args of [
    ["--write"],
    ["--install"],
    ["--unknown"],
    ["--models", "--models"],
  ]) {
    const output = run(script, args)
    assert.equal(output.code, 2)
    assert.equal(output.result.error, "invalid-arguments")
  }
  const output = run(script, [], { NESSA_DOCS_ROOT: "" })
  assert.equal(output.code, 2)
  assert.equal(output.result.error, "renderer-required")
})

async function fixture(body) {
  const project = await mkdtemp(join(tmpdir(), "nessa-state-test-"))
  try {
    await mkdir(join(project, "scripts/state-docs"), { recursive: true })
    await mkdir(join(project, "docs/state/tla"), { recursive: true })
    await cp(script, join(project, "scripts/state-docs/check.mjs"))
    const page = join(project, "docs/state/README.md")
    const markdown = `---\nid: fixture\ntitle: Test\nkind: system\nstatus: fixture\nsummary: Synthetic verification project.\nparent: null\nsources: []\ndiagramLinks: {}\n---\n\n# Test\n`
    await writeFile(page, markdown)
    await body({
      project,
      page,
      markdown,
      check: (args = [], env = {}) =>
        run(join(project, "scripts/state-docs/check.mjs"), args, env),
    })
  } finally {
    await rm(project, { recursive: true, force: true })
  }
}

test(
  "canonical catalog accepts no-model pages and rejects missing models",
  { skip: !process.env.NESSA_DOCS_ROOT },
  async () => {
    await fixture(async ({ page, markdown, check }) => {
      assert.equal(check().code, 0)
      assert.equal(check(["--models"]).result.error, "no-models")
      await writeFile(
        page,
        markdown.replace(
          "diagramLinks: {}",
          "diagramLinks: {}\nmodel:\n  spec: tla/Missing.tla\n  config: tla/Missing.cfg",
        ),
      )
      const output = check()
      assert.equal(output.code, 1)
      assert.ok(output.result.issues.some((issue) => issue.code === "missing-model-file"))
    })
  },
)

test(
  "actual TLC detects ticket reactivation and cleanup reopening without audit",
  {
    skip: !process.env.NESSA_DOCS_ROOT || process.env.NESSA_STATE_TEST_MODELS !== "1",
  },
  async () => {
    for (const [model, change, rule] of [
      [
        "mcp/ResourceTickets",
        (spec) =>
          spec.replace('phase = "Active" /\\ held', 'phase \\in {"Active", "Redeemed"}'),
        "AtMostOnce",
      ],
      [
        "sdk/Cleanup",
        (spec) =>
          spec.replace(
            '/\\ audit = "Acknowledged"\n          /\\ retired',
            '/\\ audit # "Pending"\n          /\\ retired',
          ),
        "ReopenRequiresAudit",
      ],
    ]) {
      await fixture(async ({ project, page, markdown, check }) => {
        const name = model.split("/")[1]
        const source = join(root, "docs/state/tla/services", model)
        const spec = await readFile(`${source}.tla`, "utf8")
        await writeFile(join(project, `docs/state/tla/${name}.tla`), spec)
        await cp(`${source}.cfg`, join(project, `docs/state/tla/${name}.cfg`))
        await writeFile(
          page,
          markdown.replace(
            "diagramLinks: {}",
            `diagramLinks: {}\nmodel:\n  spec: tla/${name}.tla\n  config: tla/${name}.cfg`,
          ),
        )
        const before = await readFile(page, "utf8")
        const passed = check(["--models"])
        assert.equal(passed.code, 0, JSON.stringify(passed.result))
        assert.equal(await readFile(page, "utf8"), before)
        const absent = check(["--models"], {
          NESSA_DOCS_TLC_HOME: join(project, "empty-cache"),
        })
        assert.equal(absent.code, 2)
        assert.equal(absent.result.checker.state, "not-installed")
        const saved = check(["--models", "--write"])
        assert.equal(saved.code, 0)
        assert.deepEqual(saved.result.saved, ["fixture"])
        const prior = await readFile(page, "utf8")
        await writeFile(join(project, `docs/state/tla/${name}.tla`), "invalid TLA+")
        const failed = check(["--models", "--write"])
        assert.equal(failed.code, 2)
        assert.equal(failed.result.models[0].state, "failed")
        assert.equal(await readFile(page, "utf8"), prior)
        const mutated = change(spec)
        assert.notEqual(mutated, spec, "probe must change the guard")
        await writeFile(join(project, `docs/state/tla/${name}.tla`), mutated)
        const broken = check(["--models", "--write"])
        assert.equal(broken.code, 1, JSON.stringify(broken.result))
        assert.ok(broken.result.models[0].result.broke.some((item) => item.rule === rule))
        assert.match(await readFile(page, "utf8"), /modelCheck:/)
      })
    }
  },
)
