//! Errors from serving.

/// Something went wrong accepting connections.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The listener failed.
    #[error("accepting a connection failed: {0}")]
    Io(#[from] std::io::Error),
    /// TLS could not be set up.
    #[error(transparent)]
    Tls(#[from] crate::connect::tls::Error),
}
