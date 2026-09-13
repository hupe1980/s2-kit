//! Structured logging, when the `tracing` feature is on and nothing at all when it is not.
//!
//! The drivers are the only place worth instrumenting: everything else is a pure function
//! of its arguments, and a caller who wants to know what the validator decided already has
//! the [`Report`](crate::validate::Report). What a driver knows and nobody else does is
//! *what happened on the wire and when* — which is exactly what you need at three in the
//! morning when a charger in somebody's garage will not pair.
//!
//! The macros below compile to nothing without the feature, so the call sites can be
//! unconditional and there is no `#[cfg]` threaded through the drivers.
//!
//! Every call site is inside a driver (`io`, `connect::client`, `connect::server`), so a
//! build without any driver feature — the `no_std` core, which is the build a constrained
//! Resource Manager makes — contains no call sites at all. That is not dead code by
//! mistake; it is the module doing its job, and the `allow`s below say so rather than
//! letting `-D warnings` fail a build that is exactly right.

/// Something a fleet operator would want in a log.
#[allow(
    unused_macros,
    reason = "no call sites in a build with no driver feature"
)]
macro_rules! event {
    ($level:ident, $($rest:tt)*) => {{
        #[cfg(feature = "tracing")]
        ::tracing::$level!($($rest)*);
        // Without the feature, the arguments are still type-checked against nothing —
        // which is the point: a call site that would not compile with `tracing` on must
        // not compile with it off either.
        #[cfg(not(feature = "tracing"))]
        {
            $crate::trace::ignore(format_args!(""));
        }
    }};
}

#[allow(
    unused_imports,
    reason = "no call sites in a build with no driver feature"
)]
pub(crate) use event;

/// Swallows the arguments when the feature is off.
#[cfg(not(feature = "tracing"))]
#[inline(always)]
#[allow(dead_code, reason = "no call sites in a build with no driver feature")]
pub(crate) fn ignore(_: core::fmt::Arguments<'_>) {}
