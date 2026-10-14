const urlChecks = new Set(["perf-budget", "committed-transcript"])

/** Build child arguments once, so explicit URL routing is testable without a browser. */
export function argsForCheck(check, options, { base = [], out, page, sharesPage }) {
  const args = [...base, "--out", out]
  if (!sharesPage) {
    if (urlChecks.has(check) && options.url) args.push("--url", options.url)
    if (options.runs) args.push("--runs", options.runs)
    if (check === "message-sync" || check === "committed-transcript")
      args.push("--mode", "prod")
    return args
  }

  args.push("--url", page.url)
  if (page.mode === "dev" || page.mode === "prod") args.push("--mode", page.mode)
  return args
}
