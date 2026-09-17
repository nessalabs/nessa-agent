import { pathToFileURL } from "node:url"

/** Prepare the managed background runtime only on hosts that ship that capability. */
export async function prepareDesktopRuntime({
  platform = process.platform,
  loadManagedRuntime = () => import("./prepare-macos.mjs"),
} = {}) {
  if (platform !== "darwin") return { managedGateway: false }
  await loadManagedRuntime()
  return { managedGateway: true }
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) await prepareDesktopRuntime()
