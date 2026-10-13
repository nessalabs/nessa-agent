# Codex image input (#778)

The gateway refuses phone images before upload because the Codex binding offers only text and the OpenAI catalog has no image admission limits. The pinned Codex ACP 1.12.0 harness advertises image prompts. Reuse the existing shared ACP image transport, source seam, digest checks, budgets and lifecycle; do not add a second delivery path.

## Capability agreement

| Model image limits | Injected source | Harness image offer | Behavior |
| --- | --- | --- | --- |
| Absent | Either | Either | No image admission |
| Present | Absent | Either | No image admission |
| Present | Present | Negotiated absent | Typed refusal before source read or prompt dispatch; text remains usable |
| Present | Present | Negotiation pending | Existing shared admission may read bytes; worker waits for negotiation and refuses dispatch if absent |
| Present | Present | Negotiated present | Read verified bytes through the existing bounded source and send image content blocks in attachment order |

The conversation capability also intersects the attachment owner's current upload
configuration. A present attachment port can keep and release non-image files
while its shared normalizer offers no images.

| Shared upload normalizer | Model and harness offer images | Conversation behavior |
| --- | --- | --- |
| No image limits across configured models | Yes | agents.list model imageInput false and conversation imageInput false; image ticket refused; text usable |
| Shared image limits available | Yes | agents.list model imageInput true for models with recorded limits, conversation imageInput true; image ticket admitted |

Codex construction derives input modality from `AcpConfig.images`, matching Claude's existing ownership. Runtime negotiation remains the shared worker's responsibility. Only GPT-5.6 Terra gains a conservative admission profile: PNG/JPEG, 5,000,000 encoded bytes and a 2048-pixel edge. These are Nessa's chosen delivery ceilings, not a claim about the provider's maximum; other OpenAI models remain unoffered until a profile is verified. Source: https://developers.openai.com/api/docs/models/gpt-5.6-terra (image input).

## Verification

Use the production Codex binding against the pinned-shape local fixture for each negotiated capability-table row, exact content and attachment order, image-only input, corrupt bytes and continued text execution. Unknown negotiation stays on the existing shared worker path; this slice does not change restoration or admission ordering. Add a catalog regression for the Terra profile. Revert the binding input and remove the catalog profile separately: each regression must fail. Then send a synthetic PNG through the native Swift client, authenticated upload ticket and real Codex process; assert a photo history reference, cached local image and a response identifying the fixture. The phone selects only models published by the gateway for new conversations; existing sessions retain their model. No persistence, permission or deletion policy changes.

Verified October 12 against source base `a2084194743e41cd9b0ffd62c6a7cb93494cc77f` and pinned Codex ACP 1.12.0. The native Swift client selected Terra from `agents.list`, uploaded a synthetic PNG through the ticketed HTTP path, sent its reference, read the completed history and retrieved its local cached bytes. The real model described the blue background, red circle on the left and yellow square on the right. The disposable conversation was deleted after the check. No personal image was uploaded. This validates one synthetic PNG; physical-device camera, dictation, and other image formats remain outside that live check.

The full SDK check passed 1,190 library, 444 application, 257 domain, 67 infrastructure and 39 doc tests, with existing ignored tests. All-target SDK Clippy with warnings denied, formatting, documentation checks and the gateway build passed. Reverting the binding input and removing the Terra profile independently failed their intended regressions. A fresh independent implementation review found no actionable findings. The project-board CLI token lacks project scope; issue tracking is retained through #778.
