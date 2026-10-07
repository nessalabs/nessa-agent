/**
 * The seeded page the workspace-load check opens. The numbers are the dry
 * run in `seeded-workspace.test.ts`. The page's parser owns whether that
 * query is a run; this module only writes it.
 */

export const seededLoadSpec = {
  seed: 590,
  now: 1_700_000_000_000,
  sessions: 10_000,
  longTranscripts: 1,
  messages: 8,
  messageCharacters: 4_000,
}

/** The query `seededWorkspaceSpec` reads. `seeded` is the seed. No `gateway`. */
export function seededSearch(spec) {
  const params = new URLSearchParams()
  params.set("seeded", String(spec.seed))
  params.set("now", String(spec.now))
  params.set("sessions", String(spec.sessions))
  params.set("longTranscripts", String(spec.longTranscripts))
  params.set("messages", String(spec.messages))
  params.set("messageCharacters", String(spec.messageCharacters))
  return `?${params.toString()}`
}

/** `url` with the seeded query added. An existing `gateway` param is removed. */
export function withSeeded(url, spec) {
  const next = new URL(url)
  next.searchParams.delete("gateway")
  const params = new URLSearchParams(seededSearch(spec).slice(1))
  for (const [key, value] of params) next.searchParams.set(key, value)
  return next.toString()
}
