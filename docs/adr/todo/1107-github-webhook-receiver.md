# 1107 — Receive GitHub App events on Cloudflare Free

Issue: [#1107](https://github.com/nessalabs/nessa-agent/issues/1107).
Decision: accepted for the initial receiver. Implementation and live verification
status are recorded in the issue; desktop routing is outside this slice.

## Ownership

`packages/github-webhooks` owns ingress for the nessalabs-owned Nessa Agent app.
Cloudflare Workers terminates HTTPS; D1 owns one atomic inbox. The desktop and
gateway do not import Cloudflare APIs. A later authenticated consumer needs its
own user/installation ownership and Cedar authorization contract before exposing
any retained event. This service has no public event-read API and runs no agents.

Use Workers Free and D1 only. No queue subscription, paid plan, or always-running
server is needed to retain the first deliveries. GitHub signs requests with a
dedicated webhook secret; its App private key is unnecessary for ingress and is
not deployed with this Worker.

## States and orderings

The key is GitHub's delivery UUID, canonicalized to lowercase. The stored event
name and SHA-256 of the original bytes define whether a repeated ID agrees.
Signature verification authenticates the raw bytes. GitHub's delivery/event
headers are not signed, so an actor with a captured valid webhook can replay it
under another UUID; this receiver does not execute side effects from an event.

| Before | Input / ordering | After and response |
| --- | --- | --- |
| absent | malformed headers, oversized body, wrong MAC, invalid JSON | unchanged; 400/413/401/400 |
| absent, room available | signed JSON object, SQLite insert commits | retained; 202 |
| absent | database write fails or its answer is lost | unconfirmed; 503; same UUID can be redelivered |
| retained | same UUID, event, and bytes, including concurrent arrival | unchanged; 200 |
| retained | same UUID with changed event or bytes | unchanged; 409 |
| absent, inbox full | new UUID | unchanged; 503; no eviction |
| retained, inbox full | identical duplicate | unchanged; 200 |
| one slot left | concurrent new UUIDs | one insert commits; other refuses with 503 |
| any | read route requested | unchanged; 404 |

The SQLite INSERT, conditional capacity predicate, uniqueness constraint, and
accounting triggers form one transaction. The service returns 2xx only after a
confirmed insert or a matching durable receipt. A lost response is not proof of
storage failure. The receipt retains the complete signed payload, event, digest,
delivery identity, and observation time; logs contain only ID, event, and result.

## Bounds and lifecycle

Requests are streamed up to one MiB before verification/parsing. The inbox holds
at most 10,000 deliveries and 64 MiB of raw payload bytes; constants belong to the
domain and are bound into the INSERT. Index/schema overhead is additional but
bounded by that row count. Counter checks are O(1), avoiding a full-table scan
per webhook against D1's daily read allowance. Local integration tests exercise
the real SQLite triggers, both capacity limits, and a last-slot race.

There is no automatic expiry or eviction: accepted data stays until an operator
exports and explicitly deletes it. At capacity, new deliveries fail closed. This
is a bounded starter inbox, not a production delivery pipeline with automatic
recovery. Monitor usage and failed GitHub deliveries; GitHub does not retry them
automatically. Acknowledge that limitation before enabling high-volume installs.
The deployment runbook lives in the package README.

Deleting this capability removes this package and its Worker/D1 resources. There
are no changes to desktop protocol, conversation behavior, or gateway state;
conversation stress and browser UI checks do not exercise this ingress boundary.
Worker/SQLite integration tests and a real GitHub ping verify it instead.

## Alternatives

An always-running Rust service costs more at idle and adds host maintenance.
Cloudflare Queues would add another durable boundary before any consumer exists;
D1 is already the durable acceptance receipt. Building desktop retrieval now
would require deciding multi-user installation access before its owner exists.
