//! One callsite for every operational limit a refusal or a silent close hits.
//! The wire response and close code stay what they were.
//!
//! This is not `tracing::warn!`. That macro puts every limit on one callsite,
//! and the first thread to record it — a test with no subscriber — disables
//! the callsite for the process. A later test then sees an empty log. Dispatch
//! asks the subscriber that is current on this thread.
//!
//! The function lives here, under process infrastructure, so a conversation
//! read and a product socket can both name a limit without either module
//! importing the other.

/// Name the operational limit a refusal or a silent close hit.
pub(crate) fn note_limit(limit: &'static str) {
    static CALLSITE: tracing::callsite::DefaultCallsite =
        tracing::callsite::DefaultCallsite::new(&META);
    static META: tracing::Metadata<'static> = tracing::Metadata::new(
        "product session hit an operational limit",
        "nessa_server::product::state",
        tracing::Level::WARN,
        Some(file!()),
        Some(line!()),
        Some(module_path!()),
        tracing::field::FieldSet::new(
            &["message", "limit"],
            tracing::callsite::Identifier(&CALLSITE),
        ),
        tracing::metadata::Kind::EVENT,
    );

    tracing::dispatcher::get_default(|dispatch| {
        if !dispatch.enabled(&META) {
            return;
        }
        let fields = META.fields();
        let message_field = fields
            .field("message")
            .expect("message is one of the limit event's fields");
        let limit_field = fields
            .field("limit")
            .expect("limit is one of the limit event's fields");
        let message = "product session hit an operational limit";
        let values = [
            (&message_field, Some(&message as &dyn tracing::field::Value)),
            (&limit_field, Some(&limit as &dyn tracing::field::Value)),
        ];
        let values = fields.value_set(&values);
        dispatch.event(&tracing::Event::new(&META, &values));
    });
}
