//! Desktop gateway context: application coordinates a registered background service;
//! infrastructure implements native registration, readiness and agent-stop requests.
//! The domain holds what the registration decides rather than performs — today the
//! search paths the service and its agent are given.
//!
//! ```text
//! main (composition) -> application::Gateway -> application::GatewayHost
//!                                            -> application::LoginShellPath
//! infrastructure::current() -----------------> GatewayHost implementation
//! infrastructure::login_shell_path() --------> LoginShellPath implementation
//!                            both speak in ---> domain::value_objects::SearchPath
//! ```
//! Arrows mean construction or calls. The desktop does not own gateway lifetime.
pub mod application;
pub mod domain;
pub mod infrastructure;
