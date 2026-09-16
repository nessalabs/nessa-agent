import { spawnSync } from "node:child_process"

const result = spawnSync("cargo", ["doc", "-p", "nessa-sdk", "--no-deps"], {
  env: { ...process.env, RUSTDOCFLAGS: "-D warnings" },
  stdio: "inherit",
})
if (result.error) throw result.error
if (result.status !== 0) process.exit(result.status ?? 1)
