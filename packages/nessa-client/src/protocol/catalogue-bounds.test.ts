import { describe, expect, it } from "vitest"
import { catalogueWireSchemas } from "../generated/product.js"
import {
  maxCatalogueEntries,
  maxCataloguePayloadBytes,
} from "./catalogue-read-validate.js"

describe("catalogue bounds", () => {
  it("publishes the page and payload maxima from the wire schema", () => {
    expect(maxCatalogueEntries).toBe(
      catalogueWireSchemas.CatalogueManifestRequest.properties.maxEntries.maximum,
    )
    expect(maxCataloguePayloadBytes).toBe(
      catalogueWireSchemas.ConversationCatalogueResolveParams.properties.maxPayloadBytes
        .maximum,
    )
  })
})
