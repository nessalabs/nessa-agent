#![allow(dead_code)]
mod webhooks {
 pub mod domain {
  #[path="/tmp/independent-shared-reset-review/crates/nessa-server/src/webhooks/domain/value_objects/delivery.rs"] mod delivery;
  #[path="/tmp/independent-shared-reset-review/crates/nessa-server/src/webhooks/domain/value_objects/destination.rs"] mod destination;
  pub use delivery::*; pub use destination::*;
 }
 pub mod application {
  use nessa_auth::domain::{CredentialId,OrganizationId,PrincipalId};
  #[derive(Clone, PartialEq, Eq)] pub struct Target {pub credential:CredentialId,pub principal:PrincipalId,pub organization:OrganizationId,pub owner:PrincipalId}
  #[path="/tmp/independent-shared-reset-review/crates/nessa-server/src/webhooks/application/message.rs"] mod message;
  pub use message::*;
  #[path="/tmp/independent-shared-reset-review/crates/nessa-server/src/webhooks/application/outbox.rs"] mod outbox;
  pub use outbox::*;
 }
 pub mod infrastructure {
  #[path="/tmp/independent-shared-reset-review/crates/nessa-server/src/webhooks/infrastructure/outbox/mod.rs"] pub mod outbox;
 }
}
