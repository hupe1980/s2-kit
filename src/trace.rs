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
        #[cfg(not(feature = "tracing"))]
        $crate::trace::never_called(|| {
            $crate::trace::name_only!($($rest)*);
        });
    }};
}

/// Compiles a log call's arguments without running them.
///
/// Takes the closure and drops it. The body is type-checked, so a field naming something
/// that does not exist is still a compile error and a variable mentioned only in a log
/// call still counts as used — but nothing is called, so nothing is evaluated. That second
/// half matters: `diagnostic = ?report.diagnostic_label()` allocates a `String`, and a
/// build with `tracing` off must pay for it exactly what it asked to pay, which is nothing.
#[cfg(not(feature = "tracing"))]
#[inline(always)]
#[allow(dead_code, reason = "no call sites in a build with no driver feature")]
pub(crate) fn never_called(_: impl FnOnce()) {}

/// Names each value of a `tracing` field list, in the three spellings `tracing` accepts —
/// `field = %display`, `field = ?debug` and `field = value` — followed by the message.
///
/// Only reachable from [`event!`], and only inside the closure above.
#[allow(
    unused_macros,
    reason = "no call sites in a build with no driver feature"
)]
macro_rules! name_only {
    () => {};
    ($message:literal $(,)?) => {};
    ($field:ident = % $value:expr, $($rest:tt)*) => {
        let _ = &$value;
        $crate::trace::name_only!($($rest)*);
    };
    ($field:ident = ? $value:expr, $($rest:tt)*) => {
        let _ = &$value;
        $crate::trace::name_only!($($rest)*);
    };
    ($field:ident = $value:expr, $($rest:tt)*) => {
        let _ = &$value;
        $crate::trace::name_only!($($rest)*);
    };
}

#[allow(
    unused_imports,
    reason = "no call sites in a build with no driver feature"
)]
pub(crate) use {event, name_only};
