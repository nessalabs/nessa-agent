import { copyFile, mkdir } from "node:fs/promises"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
const target = new URL("../src/relay/infrastructure/cedar/generated/", import.meta.url)
await mkdir(target, { recursive: true })
await copyFile(
  join(
    dirname(fileURLToPath(import.meta.resolve("@cedar-policy/cedar-wasm/web"))),
    "cedar_wasm_bg.wasm",
  ),
  new URL("cedar.wasm", target),
)
