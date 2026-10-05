import { resolve } from "node:path"

/** Read the built app's effective declarations, rather than trusting source plists. */
export function verifyTerminalAutomation(app, run) {
  const entitlementXml = run(
    "/usr/bin/codesign",
    ["--display", "--entitlements", "-", "--xml", app],
    { encoding: "utf8" },
  )
  const entitlements = JSON.parse(
    run("/usr/bin/plutil", ["-convert", "json", "-o", "-", "-"], {
      encoding: "utf8",
      input: entitlementXml,
    }),
  )
  const info = JSON.parse(
    run(
      "/usr/bin/plutil",
      ["-convert", "json", "-o", "-", resolve(app, "Contents/Info.plist")],
      { encoding: "utf8" },
    ),
  )
  if (
    !Object.hasOwn(entitlements, "com.apple.security.automation.apple-events") ||
    entitlements["com.apple.security.automation.apple-events"] !== true
  )
    throw new Error("The signed app does not permit requesting Terminal automation")
  if (
    !Object.hasOwn(info, "NSAppleEventsUsageDescription") ||
    typeof info.NSAppleEventsUsageDescription !== "string" ||
    !info.NSAppleEventsUsageDescription.trim()
  )
    throw new Error("The built app does not explain its Terminal automation request")
}
