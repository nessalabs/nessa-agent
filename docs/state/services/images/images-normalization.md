---
id: images-normalization
title: Normalization pipeline
kind: operation
status: implemented
summary: "The image library checks the bytes, handles orientation and fits the image to the requested limits."
parent: images
sources:
  - crates/nessa-images/src/normalize.rs
  - crates/nessa-images/src/lib.rs
diagramLinks: {}
---

# Normalization pipeline

The image library checks the bytes, handles orientation and fits the image to the requested limits. It may keep an already suitable image, resize it, or encode it as PNG or JPEG.

Decoding, pixel count and memory use are bounded. Unsupported formats or images that cannot fit are refused. Platform decoders can add format support, but that support is not available on every operating system. This library does no storage or network work and has no persistent lifecycle.

```mermaid
stateDiagram-v2
    [*] --> Inspecting: normalize_with / sniff and read header
    Inspecting --> Validating: acceptable encoding, dimensions and bytes
    Validating --> Unchanged: whole decode succeeds and single frame / return original bytes
    Inspecting --> Decoding: fitting required [budgets permit]
    Validating --> Decoding: strict JPEG validation fails [forgiving decode possible]
    Decoding --> Fitting: decode succeeds / orient and choose encoding
    Fitting --> Complete: encoded bytes fit
    Fitting --> Fitting: too large [next size remains usable] / reduce quality or scale
    Inspecting --> Refused: unsupported or oversized input
    Decoding --> Refused: undecodable or memory budget exceeded
    Fitting --> Refused: cannot fit usable size
```

## Further reading

[Source](../../../../crates/nessa-images/src/normalize.rs) · [Related source](../../../../crates/nessa-images/src/lib.rs)
