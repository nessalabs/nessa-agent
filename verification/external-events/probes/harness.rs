// Test-only application-port substitutes. The HTTP adapter itself is copied from source.
extern crate self as nessa_auth;
extern crate self as nessa_sdk;
pub mod application {
    pub mod ports {
        #[derive(Debug)]
        pub enum AccessError {
            Unavailable,
            Denied,
        }
        pub trait Clock: Send + Sync {
            fn unix_seconds(&self) -> u64;
        }
    }
    pub mod agent_execution {
        pub mod commands {
            #[derive(Debug)]
            pub enum MutationFailure<T> {
                Conflict,
                Interrupted(Box<()>),
                Other(T),
            }
        }
    }
}
pub mod external_events {
    pub mod domain {
        #[path = "@IDENTITY_SOURCE@"]
        mod identity;
        pub use identity::EventId;
    }
    pub mod application {
        use super::domain::EventId;
        use crate::application::{
            agent_execution::commands::MutationFailure,
            ports::{AccessError, Clock},
        };
        use std::sync::Arc;
        pub struct Gate {
            pub entered: std::sync::atomic::AtomicUsize,
            pub released: tokio::sync::watch::Sender<bool>,
        }
        pub struct EventBinding {
            pub id: EventId,
            pub clock: Arc<dyn Clock>,
            pub gate: Option<Arc<Gate>>,
        }
        pub struct Admission {
            pub execution_id: String,
        }
        pub enum AdmissionFailure {
            Access(AccessError),
            Submit(MutationFailure<()>),
            NotDispatched,
        }
        impl EventBinding {
            pub async fn admit(
                &self,
                event: &EventId,
                _text: String,
            ) -> Result<Admission, AdmissionFailure> {
                if let Some(gate) = &self.gate {
                    let mut released = gate.released.subscribe();
                    gate.entered
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    released.wait_for(|released| *released).await.unwrap();
                }
                Ok(Admission {
                    execution_id: event.as_str().into(),
                })
            }
        }
    }
    pub mod entrypoint {
        #[path = "@HTTP_COPY@"]
        pub mod http;
    }
}
#[cfg(test)]
mod allocation_probe {
    use std::alloc::{GlobalAlloc, Layout, System};
    pub static MAX: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    struct Tracked;
    unsafe impl GlobalAlloc for Tracked {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            MAX.fetch_max(layout.size(), std::sync::atomic::Ordering::SeqCst);
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
            unsafe { System.dealloc(p, layout) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            MAX.fetch_max(n, std::sync::atomic::Ordering::SeqCst);
            unsafe { System.realloc(p, l, n) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: Tracked = Tracked;
}
