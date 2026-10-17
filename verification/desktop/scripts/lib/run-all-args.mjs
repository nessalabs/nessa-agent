const productionFixtures = new Set(["committed-transcript", "settings-page"])
const urlChecks = new Set(["perf-budget", ...productionFixtures])

/** Checks whose fixture inputs require a separate production build. */
export function ownsPreview(check, mode) {
  return (
    check === "message-sync" ||
    check === "perf-budget" ||
    (mode === "prod" && productionFixtures.has(check))
  )
}

/** Build child arguments once, so explicit URL routing is testable without a browser. */
export function argsForCheck(check, options, { base = [], out, page, sharesPage }) {
  const args = [...base, "--out", out]
  if (!sharesPage) {
    if (urlChecks.has(check) && options.url) args.push("--url", options.url)
    if (check === "perf-budget" && options.runs) args.push("--runs", options.runs)
    if (check === "message-sync" || productionFixtures.has(check))
      args.push("--mode", "prod")
    return args
  }

  args.push("--url", page.url)
  if (page.mode === "dev" || page.mode === "prod") args.push("--mode", page.mode)
  return args
}
