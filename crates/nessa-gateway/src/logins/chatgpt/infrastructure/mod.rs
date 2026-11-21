//! OpenAI's sign-in over the network, and the address on this computer the
//! browser comes back to.
pub mod authority;
pub mod callback;

pub use authority::{ChatgptSignInConfig, OpenaiSignIn};
pub use callback::LoopbackListener;
