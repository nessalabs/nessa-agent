# 921. Close product method classification at the gateway

## Purpose

Make method-extension omissions visible before they reach the gateway. This
selects an implementation direction for #921; runtime migration remains future
work in `todo/`.

- **Date:** 2026-10-15
- **Status:** proposed

## Context

The manifest owns method names, parameter schemas and requested grants. The
gateway separately selects handlers, response lanes, execution pools and paired
admission through strings and defaults. Its advertised-method membership test
does not establish runtime dispatch. No currently advertised missing handler has
been demonstrated. The [source catalogue and implementation plan](../../design/product-method-classification.md)
records the reviewed paths and the proposed migration.

## Decision

Generate a closed Rust `ProductMethod` identity from the existing manifest and
parse requests at the socket boundary. Keep one exhaustive gateway-owned
`MethodPlan` selecting handler, response lane, execution capacity and admission
plan; keep these server choices out of the protocol manifest. The admission
plan selects existing owner facades, including compound manifest-grant plus
read or paired Share/Unshare/Shares stages; it introduces no universal
authorization executor. Cedar keeps policy verdicts; existing application and lifecycle owners retain admission and permit
ownership. Cut over consumers and remove replaced classifiers together, preserving
unknown-method refusal order, handshake phases and independent control capacity.

## Alternatives considered

- **Expand the manifest or generate the server plan:** server routing and
  capacity belong to the gateway; publishing them would invert ownership.
- **Keep strings with stronger enumeration tests:** defaults can still conceal
  omitted choices. Closed identity and exhaustive construction expose omissions.
- **Introduce a command bus or permanent authorization capability:** extension
  completeness needs neither; grant freshness remains with admission owners.

## Consequences and proof

New methods require an explicit plan. A synthetic extension compiled against the
real classifier must fail until classified; required fields establish plan
completeness. Per-method handler and socket tests establish execution and
semantic choices, which compilation alone cannot prove; it cannot detect an
unmodeled future policy stage. The companion owns the fixture mechanics, expectation catalogue, ordering matrix and bounded migration.
This accepts an inert generated identity followed by one gateway cutover, with
more explicit extension work in exchange for deleting distributed defaults.
Runtime behavior and wire contract remain unchanged by this design record.
