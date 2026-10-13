//! Conversation access, in Cedar: composition builds
//! [`CedarConversationAccessPolicy`] once and injects it as the application's
//! `ConversationAccessPolicyEvaluator`; nothing else names Cedar.
//!
//! ```text
//! composition ──▶ CedarConversationAccessPolicy::new()
//!                   loads and strictly validates schema.json with
//!                   ownership.cedar + shares.cedar + readers.cedar,
//!                   every policy carrying an @id
//! application ──▶ ConversationAccessPolicyEvaluator::evaluate(AccessAsk)
//!                   evaluator.rs: the subject ──▶ a Person, Reader or
//!                   GrantHolder entity; the resource ──▶ a Conversation or
//!                   Catalogue entity; the role held, the request's shape
//!                   and a share's target ──▶ the context
//!                   ──▶ Cedar Authorizer ──▶ Allow, with the @ids that
//!                   allowed it | Deny: Forbidden by @ids, or NotPermitted
//!                   ──▶ an error on any step: logged, Deny Unavailable
//! ```
//!
//! Arrows are construction and calls. Cedar sees facts only: who the
//! subject is and whom it signed in as, the conversation's organization,
//! owner and deletion, the role a grant holds as stored, what a command
//! carries, and what a share target holds. Role ordering, ownership and the
//! method rules are the policies'. A bundle that does not validate fails
//! construction; an entity, a request, or an evaluation that errors is
//! logged and denies.
mod evaluator;

pub use evaluator::CedarConversationAccessPolicy;
