import contract from "../../../../../protocol/defaults/github-relay-contract.json"
export const relayLimits = Object.freeze({
  pageSize: contract.pageSize,
  pageBytes: contract.payloadBytes,
  retentionMs: contract.retentionSeconds * 1000,
  ticketMs: contract.ticketSeconds * 1000,
  auditRows: contract.auditRows,
  sweepRows: contract.sweepRows,
})
export function repositoryIdentity(value: string | null): string | null {
  return typeof value === "string" &&
    /^[1-9][0-9]{0,15}$/.test(value) &&
    Number.isSafeInteger(Number(value))
    ? value
    : null
}
export function relayCursor(value: string | null): number | null {
  if (value === null || !/^(0|[1-9][0-9]{0,15})$/.test(value)) return null
  const cursor = Number(value)
  return Number.isSafeInteger(cursor) ? cursor : null
}
