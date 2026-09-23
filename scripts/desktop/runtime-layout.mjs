const POSIX_EXECUTABLES = Object.freeze({
  node: "node",
  gateway: "nessa",
  mcp: "nessa-mcp",
})
const WINDOWS_EXECUTABLES = Object.freeze({
  node: "node.exe",
  gateway: "nessa.exe",
  mcp: "nessa-mcp.exe",
})

/** Name the executables produced and bundled on one desktop platform. */
export function runtimeExecutables(platform) {
  if (platform === "darwin" || platform === "linux") return POSIX_EXECUTABLES
  if (platform === "win32") return WINDOWS_EXECUTABLES
  throw new Error(`Bundled gateway packaging does not support ${platform}`)
}
