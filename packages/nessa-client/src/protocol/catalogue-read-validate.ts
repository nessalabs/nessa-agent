import { bounds, catalogueWireSchemas } from "../generated/product.js"
import type {
  ConversationCatalogueHeadResult,
  ConversationCatalogueManifestResult,
  ConversationCatalogueResolveResult,
} from "../generated/product.js"
import {
  canonicalBase64,
  decimal,
  validPositiveReadEpoch,
} from "./passive-read-validate.js"

interface Shape {
  type?: string
  $ref?: string
  properties?: Readonly<Record<string, Shape>>
  required?: readonly string[]
  items?: Shape
  enum?: readonly string[]
  additionalProperties?: boolean
  minLength?: number
  maxLength?: number
  pattern?: string
  minimum?: number
  maximum?: number
  minItems?: number
  maxItems?: number
  "x-utf8MaxBytes"?: number
}
const UTF8 = new TextEncoder()
const shapes: Readonly<Record<string, Shape>> = catalogueWireSchemas

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

/** Representation only: sync-engine owns pass ordering and current-value semantics. */
function matches(value: unknown, shape: Shape): boolean {
  if (shape.$ref) {
    const name = shape.$ref.split("/").at(-1)
    return (
      name !== undefined && Object.hasOwn(shapes, name) && matches(value, shapes[name])
    )
  }
  switch (shape.type) {
    case "object": {
      if (!object(value) || !shape.properties) return false
      const properties = shape.properties
      if (shape.required?.some((key) => !Object.hasOwn(value, key))) return false
      return Object.keys(value).every(
        (key) => Object.hasOwn(properties, key) && matches(value[key], properties[key]),
      )
    }
    case "string":
      return (
        typeof value === "string" &&
        (shape.enum === undefined || shape.enum.includes(value)) &&
        (shape.minLength === undefined || [...value].length >= shape.minLength) &&
        (shape.maxLength === undefined || [...value].length <= shape.maxLength) &&
        (shape["x-utf8MaxBytes"] === undefined ||
          UTF8.encode(value).length <= shape["x-utf8MaxBytes"]) &&
        (shape.pattern === undefined || new RegExp(shape.pattern).test(value)) &&
        (shape.pattern !== bounds.decimalU64Pattern || decimal(value)) &&
        (shape.pattern !== bounds.positiveEpochPattern || validPositiveReadEpoch(value))
      )
    case "integer":
      return (
        typeof value === "number" &&
        Number.isSafeInteger(value) &&
        (shape.minimum === undefined || value >= shape.minimum) &&
        (shape.maximum === undefined || value <= shape.maximum)
      )
    case "boolean":
      return typeof value === "boolean"
    case "array": {
      const itemShape = shape.items
      return (
        Array.isArray(value) &&
        (shape.minItems === undefined || value.length >= shape.minItems) &&
        (shape.maxItems === undefined || value.length <= shape.maxItems) &&
        itemShape !== undefined &&
        value.every((item) => matches(item, itemShape))
      )
    }
    default:
      return false
  }
}

export function checkedCatalogueShape<T>(
  value: unknown,
  name: keyof typeof catalogueWireSchemas,
): T {
  if (!Object.hasOwn(shapes, name) || !matches(value, shapes[name]))
    throw new TypeError(`Invalid catalogue transport shape: ${name}`)
  return value as T
}

export function catalogueHead(value: unknown): ConversationCatalogueHeadResult {
  return checkedCatalogueShape(value, "ConversationCatalogueHeadResult")
}
export function catalogueManifest(value: unknown): ConversationCatalogueManifestResult {
  return checkedCatalogueShape(value, "ConversationCatalogueManifestResult")
}

/** Decoded bytes and exact echoed request; callers apply engine semantic validation before commit. */
export type DecodedCatalogueResolve = Omit<
  ConversationCatalogueResolveResult,
  "payload"
> & { payload: Uint8Array }
export function catalogueResolved(value: unknown): DecodedCatalogueResolve {
  const response = checkedCatalogueShape<ConversationCatalogueResolveResult>(
    value,
    "ConversationCatalogueResolveResult",
  )
  return {
    ...response,
    payload: canonicalBase64(response.payload, bounds.maxRecordResponseBytes, true),
  }
}
