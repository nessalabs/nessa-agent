//! Audit failure metadata for the existing stderr log; never record contents or paths.
use std::io;

pub(super) fn observe_io<T>(
    audit: &'static str,
    phase: &'static str,
    result: io::Result<T>,
) -> io::Result<T> {
    result.inspect_err(|error| {
        tracing::error!(
            audit_kind = audit,
            audit_phase = phase,
            error_kind = ?error.kind(),
            os_error = ?error.raw_os_error(),
            "Audit persistence failed"
        );
    })
}

pub(super) fn unavailable(audit: &'static str, phase: &'static str) {
    tracing::error!(
        audit_kind = audit,
        audit_phase = phase,
        "Audit persistence failed"
    );
}
