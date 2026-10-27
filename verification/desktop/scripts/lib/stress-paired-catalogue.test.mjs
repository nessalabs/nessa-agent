import { strict as assert } from "node:assert"
import { execFile } from "node:child_process"
import { mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { delimiter, join } from "node:path"
import { test } from "node:test"
import { promisify } from "node:util"

const execute = promisify(execFile)

for (const [name, code, summary] of [
  ["a zero-match successful process", 0, "ok. 0 passed; 0 failed;"],
  ["a failed process despite a passing-looking summary", 1, "ok. 1 passed; 0 failed;"],
]) {
  test(
    `native catalogue refuses ${name}`,
    { skip: process.platform === "win32" },
    async () => {
      const directory = mkdtempSync(join(tmpdir(), "native-catalogue-refusal-"))
      try {
        writeFileSync(
          join(directory, "cargo"),
          `#!${process.execPath}\nconsole.log(${JSON.stringify(`test result: ${summary}`)}); process.exit(${code});\n`,
          { mode: 0o700 },
        )
        const module = new URL("./stress-paired-catalogue.mjs", import.meta.url).href
        const script = `
        import { pairedCatalogue } from ${JSON.stringify(module)};
        try { await pairedCatalogue(1); process.exitCode = 42; }
        catch (error) {
          if (!error.message.startsWith(${JSON.stringify(code === 0 ? "native paired catalogue matched no passing test" : 'native paired catalogue {"code":1,')})) throw error;
          console.log('expected refusal');
        }
      `
        const result = await execute(
          process.execPath,
          ["--input-type=module", "-e", script],
          {
            env: { ...process.env, PATH: directory + delimiter + process.env.PATH },
            timeout: 10_000,
          },
        )
        assert.equal(result.stdout.trim(), "expected refusal")
      } finally {
        rmSync(directory, { recursive: true, force: true })
      }
    },
  )
}
