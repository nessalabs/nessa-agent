use super::service::{Evidence, RunRequest, RunResult};
use crate::shell::domain::StopCause;
use std::{future::Future, pin::Pin};
use tokio::sync::watch;

pub type Task<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Audit: Send + Sync {
    fn record<'a>(
        &'a self,
        request: &'a RunRequest,
        evidence: Evidence,
    ) -> Task<'a, Result<(), ()>>;
}
pub trait Runner: Send + Sync {
    fn run<'a>(
        &'a self,
        request: &'a RunRequest,
        stop: watch::Receiver<Option<StopCause>>,
        audit: &'a dyn Audit,
    ) -> Task<'a, RunResult>;
}
