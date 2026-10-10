# Operational limits

Each row is a limit the gateway names when a refusal or a silent close hits it. The number is the owner's, read when this document is rendered; this file is compared to that rendering and is not edited by hand. The client's preparing retries are named here and counted only in `PREPARING_ATTEMPTS` (`crates/nessa-client-core/src/read_only_sync/application/watch.rs`); this document does not copy that count. A wire field carrying the limit id is a later schema change and is not these rows.

| id | tier | effective | owner | what hitting it means |
| --- | --- | --- | --- | --- |
| product.max_payload_bytes | fixed | 65536 | protocol/product/v1.json x-frameBytes.maxPayloadBytes | a request frame longer than this is refused before it is decoded |
| product.max_record_response_bytes | fixed | 131072 | protocol/product/v1.json RecordPageResult bounds | a record page past this is refused rather than written to the socket |
| product.max_client_id_characters | fixed | 256 | protocol/product/v1.json ProductClientMetadata.id maxLength | a client id longer than this is refused at the handshake |
| product.max_surface_instance_characters | fixed | 256 | protocol/product/v1.json ProductSurface.instance maxLength | a surface instance longer than this is refused at the handshake |
| server.requests | configured | 128 | config.json limits.requests | an ordinary request past the gateway's admission is refused |
| server.controls | configured | 32 | config.json limits.controls | a control past the gateway's admission is refused |
| server.record_reads | configured | 4 | config.json limits.recordReads | a record read past the gateway's admission is refused |
| server.deletions | configured | 8 | config.json limits.deletions | a deletion past the gateway's admission is refused |
| server.upload_begins | configured | 16 | config.json limits.uploadBegins | an upload past the gateway's admission is refused |
| socket.ordinary_slots | configured | 16 | config.json limits.ordinarySlots | an ordinary frame past this socket's slots is refused |
| socket.control_slots | configured | 4 | config.json limits.controlSlots | a control frame past this socket's slots is refused |
| socket.record_slot | fixed | 1 | product socket RECORD_SLOT | a second record frame while one is in flight is refused |
| socket.app_calls | configured | 4 | config.json limits.appCallsPerSocket | an app call past this socket's lane is refused |
| socket.app_mount | configured | 3 | one less than limits.appCallsPerSocket | a mount that would take the lane's last slot is refused |
| socket.ordinary_lane | derived | 20 | ordinary slots plus app calls per socket | a response that does not fit the ordinary lane closes the socket |
| socket.control_lane | derived | 4 | control slots | a control response that does not fit the control lane closes the socket |
| socket.record_lane | fixed | 1 | product socket RECORD_LANE | a second record response while one is queued closes the socket |
| socket.refusal_lane | fixed | 1 | product socket REFUSAL_LANE | an app refusal waits while one is queued; a non-app refusal on a full lane closes the socket |
| socket.write_timeout | configured | 5000 | config.json session.writeTimeoutMs | a write that outlasts this closes the socket |
| socket.record_delivery_deadline | fixed | 30000 | RECORD_SEND_TIMEOUT, the schema's passive delivery budget | a record response still queued at this deadline is noted and dropped |
| socket.watch_delivery_deadline | fixed | 30000 | RECORD_SEND_TIMEOUT, the same duration as record delivery | a watch frame still queued at this deadline is noted and dropped |
| socket.subscription_delivery_deadline | fixed | 10000 | protocol/product/v1.json x-subscriptionLimits.deliveryTimeoutMs | a subscription frame the writer has not taken by this ends that subscription as lagging; its end frame unwritten by this closes the socket |
| socket.conversation_subscriptions | fixed | 8 | protocol/product/v1.json x-subscriptionLimits.conversationTargets | a conversation subscription past this on one socket is refused |
| socket.list_subscriptions | fixed | 1 | protocol/product/v1.json x-subscriptionLimits.listTargets | a list subscription past this on one socket is refused |
| record.change_watches | fixed | 64 | MAX_RECORD_CHANGE_WATCHES in crates/nessa-sdk/src/infrastructure/session_storage/record_changes.rs | the record watches every socket shares, devices' and subscriptions' alike; past this a watch or subscription is refused subscription_capacity. A view subscription holds one and a list subscription one (any commit), so a desktop window following its limit (8 views and the list) holds 9, and about 7 such windows fill it |
| conversation.catalogue_watches | fixed | 64 | MAX_CATALOGUE_CHANGE_WATCHES in crates/nessa-server/src/conversation/infrastructure/catalogue_changes.rs | the catalogue watches every socket shares; a list subscription holds one beside its record watch, as a device's catalogue watch does; past this one is refused subscription_capacity |
| conversation.read_grants | fixed | 64 | MAX_READ_GRANTS_PER_CONVERSATION in crates/nessa-server/src/conversation/application/read_grants.rs, the schema's ConversationSharesResult maxItems | the paired devices one conversation is shared with; one more conversation.share is refused invalid_request, so a conversation.shares answer always fits one frame |
| record.read_work_budget | configured | 200 | config.json limits.readWorkBudgetMs | a cold read still preparing at this budget answers that it is preparing |
| record.discovery_steps | fixed | 128 | record read DISCOVERY_STEPS_PER_READ | a cold read that uses every discovery step answers that the source is preparing |

| client.preparing_attempts | client | the client's own | `PREPARING_ATTEMPTS` in `crates/nessa-client-core/src/read_only_sync/application/watch.rs` | how many times a preparing read is asked again |
