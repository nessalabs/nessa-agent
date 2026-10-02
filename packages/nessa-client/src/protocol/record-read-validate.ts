import { canonicalBase64, decimal } from "./passive-read-validate.js"
import { bounds } from "../generated/product.js"
import type {
  ConversationRecordsHeadResult,
  RecordPageRequest,
  RecordScope,
} from "../generated/product.js"

const UTF8 = new TextEncoder()
function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

export function validRecordReceiverId(value: unknown): value is string {
  return typeof value === "string" && UTF8.encode(value).length <= bounds.maxSyncIdBytes
}

export function validRecordScope(value: unknown): value is RecordScope {
  return (
    object(value) &&
    Object.keys(value).length === 6 &&
    validRecordReceiverId(value.receiver) &&
    validRecordReceiverId(value.origin) &&
    validRecordReceiverId(value.stream) &&
    validRecordReceiverId(value.incarnation) &&
    validRecordReceiverId(value.schema) &&
    validRecordReceiverId(value.accessEpoch)
  )
}

function positiveBound(value: unknown, maximum: number): value is number {
  return (
    typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= 1 &&
    value <= maximum
  )
}

export function validRecordPageRequest(value: unknown): value is RecordPageRequest {
  return (
    object(value) &&
    Object.keys(value).length === 6 &&
    validRecordScope(value.scope) &&
    decimal(value.after) &&
    decimal(value.target) &&
    positiveBound(value.maxRecords, bounds.maxRecordPageRecords) &&
    positiveBound(value.maxPayloadBytes, bounds.maxPhysicalRecordPayloadBytes) &&
    positiveBound(value.maxRecordBytes, bounds.maxPhysicalRecordPayloadBytes)
  )
}

export function recordHead(value: unknown): ConversationRecordsHeadResult {
  if (
    !object(value) ||
    Object.keys(value).length !== 2 ||
    !validRecordScope(value.scope) ||
    !decimal(value.head)
  ) {
    throw new TypeError("Invalid record head response")
  }
  return value as unknown as ConversationRecordsHeadResult
}

/** Decoded physical record. The sync-engine still validates the page envelope. */
export type DecodedRecord = {
  position: string
  id: string
  payload: Uint8Array
}

/** Echo and decoded records, ready for sync-engine's public validate_page. */
export type DecodedRecordPage = {
  request: RecordPageRequest
  records: DecodedRecord[]
}

export function recordPage(value: unknown): DecodedRecordPage {
  if (
    !object(value) ||
    Object.keys(value).length !== 2 ||
    !validRecordPageRequest(value.request) ||
    !Array.isArray(value.records) ||
    value.records.length < 1 ||
    value.records.length > bounds.maxRecordPageRecords
  ) {
    throw new TypeError("Invalid record page response")
  }
  let total = 0
  const records: DecodedRecord[] = value.records.map((entry: unknown) => {
    if (
      !object(entry) ||
      Object.keys(entry).length !== 3 ||
      !decimal(entry.position) ||
      !validRecordReceiverId(entry.id)
    ) {
      throw new TypeError("Invalid physical record")
    }
    const payload = canonicalBase64(entry.payload, bounds.maxPhysicalRecordPayloadBytes)
    total += payload.length
    if (total > bounds.maxPhysicalRecordPayloadBytes)
      throw new TypeError("Record page exceeds transport byte bound")
    return { position: entry.position, id: entry.id, payload }
  })
  return { request: value.request, records }
}
