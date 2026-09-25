import { pathToFileURL } from "node:url"

/** Prepare the runtime resource on hosts with a verified native assembler. */
export async function prepareDesktopRuntime({
  platform = process.platform,
  loadMacosRuntime = async () =>
    (await import("./prepare-macos.mjs")).prepareMacosRuntime,
  loadLinuxRuntime = async () =>
    (await import("./prepare-linux.mjs")).prepareLinuxRuntime,
} = {}) {
  if (platform === "darwin") {
    const prepare = await loadMacosRuntime()
    prepare()
    return { managedGateway: true }
  }
  if (platform === "linux") {
    const prepare = await loadLinuxRuntime()
    prepare()
    return { managedGateway: false }
  }
  return { managedGateway: false }
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) await prepareDesktopRuntime()
