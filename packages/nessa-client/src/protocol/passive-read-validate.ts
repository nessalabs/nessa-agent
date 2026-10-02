import { bounds } from "../generated/product.js"
const DECIMAL_U64 = new RegExp(bounds.decimalU64Pattern)
const POSITIVE_EPOCH = new RegExp(bounds.positiveEpochPattern)
const BASE64 = new RegExp(bounds.recordPayloadPattern)
const U64_MAX = 18_446_744_073_709_551_615n
export function decimal(value: unknown): value is string {
  return (
    typeof value === "string" &&
    DECIMAL_U64.test(value) &&
    value.length <= bounds.maxDecimalU64Characters &&
    BigInt(value) <= U64_MAX
  )
}

export function validPositiveReadEpoch(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length <= bounds.maxPositiveEpochCharacters &&
    POSITIVE_EPOCH.test(value) &&
    BigInt(value) <= U64_MAX
  )
}

export function canonicalBase64(
  value: unknown,
  maximum: number,
  allowEmpty = false,
): Uint8Array {
  if (
    typeof value !== "string" ||
    (!allowEmpty && value.length < bounds.minRecordPayloadEncodedCharacters) ||
    value.length > 4 * Math.ceil(maximum / 3) ||
    !BASE64.test(value)
  ) {
    throw new TypeError("Invalid passive read payload")
  }
  let binary: string
  try {
    binary = atob(value)
  } catch {
    throw new TypeError("Invalid passive read payload")
  }
  if (btoa(binary) !== value || binary.length > maximum)
    throw new TypeError("Invalid passive read payload")
  return Uint8Array.from(binary, (character) => character.charCodeAt(0))
}
