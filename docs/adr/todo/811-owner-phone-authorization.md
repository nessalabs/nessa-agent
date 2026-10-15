# 811. First-party phones hold an explicit owner conversation capability

## Purpose

An owner authorizes their phone once to read and manage current and future owned conversations. This capability preserves the existing per-conversation sharing rules for other devices and peers. Implementation remains in progress; the Auth foundation alone does not establish a connected phone or first Send.

- **Date:** 2026-10-13
- **Status:** accepted
- **Issue:** [#811](https://github.com/nessalabs/nessa-agent/issues/811)

## Context

Existing device pairing deliberately delegates one read capability and requires a share for each conversation. A freshly created empty conversation is absent from the product summary list, although real creation already publishes an empty transcript stream and committed snapshot. Sharing a list cannot provide owner authority over future conversations. A coarse conversation write action also covers ancillary administration, so action grants alone cannot express the requested phone boundary.

## Decision

Auth owns a separate OwnerPhoneReadWrite class, its complete ordered capability set, signed exact device request and durable original-request ledger. The exact original bootstrap credential admits creation, replay, staging and publication. The issued device uses its private native key with its public credential ID for protected product and native read connections. Canonical Auth enrollment and its remembered receiver receipt durably own purpose once; current delegated Auth and current receiver agreement establish the distinct OwnerPhone reader. The purpose Cedar policy consumes published method/action facts and default-denies ancillary administration; it also preserves configured Ask approval and refuses owner-local path links. Current signed purpose and original receiver holding admit a first-party reader only after the canonical linkage policy passes. The shared ConversationAccessPolicyEvaluator then decides actual conversation Read/Act/Locate and catalogue List from current metadata, including deletion. Device and peer readers remain separate paired subjects with their existing share roles. Existing peer/device share roles, tool approval and revocation remain separate authorities.

## Alternatives considered

- Bulk sharing existing conversations misses future/empty targets and changes the meaning of shared access.
- A persistent bootstrap credential proxy gives the phone the wrong authority and prevents retirement of setup-only secrets.
- Inferring purpose from coarse grants admits ancillary runtime/workspace administration.
- Copying purpose into a changed receiver schema creates a second authority and would require replacing or migrating existing approved receiver history. The canonical Auth consent, issued device proof and remembered receiver already retain the exact linkage.

## Consequences

The gateway must publish authenticated actual native discovery and support exact original-request recovery. Native preparation must be durable before mutation, and protected product must have a separate bounded physical child whose close drains before parent disposal. Active delegated access depends on current device/member/receiver authority, while the original bootstrap remains historical linkage after retirement. Current receiver epochs may advance beyond the original Pair receipt; historical equality cannot stand in for current progress. Completion requires a genuine signed enrollment, no-share creation, positive native empty projection, first Send/reply and current revocation/close proofs. No grants-only subset is a completed implementation.


## Current integration and verification boundaries

`phone.endpoint` returns the authenticated source and a routable numeric native
endpoint/public pin. A wildcard `native.listenAddress` is published as
`native.advertisedAddress` on the actual bound port. An absent listener, port
zero, or a wildcard with no advertised address discloses nothing. `phone.authorize` and `phone.status` consume the complete
original signed request through the canonical Auth ledger; original TLS recovery
uses the saved UUID/digest and actual device proof without restoring a PAKE
attempt. Mutating resume owns expiry and late receiver Pair/fence settlement;
read-only recovery never expires or issues.

The receiver policy fingerprint includes the Auth profile, purpose bundle and
all shared conversation-access schema/policy sources through one length-delimited
hash owner. The actual receiver journal advances its policy epoch before saved
progress is reused. Ordinary paired catalogue filtering retains its continuation;
a first-party candidate contradiction refuses the complete pass without a new
cursor or a disclosed denied ID.

Current development contracts are used directly. There are no schema aliases or
old-format fallback paths for this feature; any stale development data migration
must preserve existing chats and drafts and cannot manufacture authorization.
The server source/fixture proofs do not establish Apple package adoption or a
connected phone. Actual protected-product composition, positive fresh-empty
native projection and first Send/reply remain required before completion.
