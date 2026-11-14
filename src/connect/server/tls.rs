//! Serving the router over TLS.
//!
//! `axum` has no opinion about TLS and neither does this crate, beyond the one thing S2
//! Connect makes non-negotiable: the certificate a LAN endpoint serves is the one clients
//! bind their challenge responses to, so it cannot be changed while an endpoint is up
//! without breaking every pairing in flight.

use alloc::sync::Arc;

use tokio::net::TcpListener;

use super::tls_error::Error;

/// The most connections served at once; see [`serve`].
pub const MAX_CONNECTIONS: usize = 256;

/// How long a client may take over the TLS handshake, and over a request's headers.
pub const HANDSHAKE_TIMEOUT: core::time::Duration = core::time::Duration::from_secs(10);

/// Serve a router over TLS until the future returned by `shutdown` completes.
///
/// One task per connection, `http1` and `h2` both negotiated through ALPN. A handshake
/// that fails is dropped silently: that is a port scan, a browser, or a client with the
/// wrong pin, and none of them is worth stopping the endpoint for.
///
/// Bounded, because the pairing routes need no credentials and an embedded CEM has little
/// to spare (`S2C §DDoS countermeasures`): at most [`MAX_CONNECTIONS`] at once — further
/// connections are closed on accept — and [`HANDSHAKE_TIMEOUT`] for the TLS handshake and
/// for a request's headers, so a client that opens a socket and says nothing slowly
/// (slowloris) holds a slot for seconds rather than for ever.
///
/// ```no_run
/// use s2_kit::connect::server::serve;
/// use s2_kit::connect::tls::SelfSignedEndpoint;
///
/// # async fn run(router: axum::Router) -> Result<(), Box<dyn std::error::Error>> {
/// let identity = SelfSignedEndpoint::generate(["EVSE1038.local".into()])?;
/// let listener = tokio::net::TcpListener::bind("0.0.0.0:443").await?;
///
/// // The fingerprint every pairing client will bind to. Hand it to `PairingService::lan`.
/// let leaf = identity.leaf;
/// serve(listener, identity.server_config()?, router, std::future::pending()).await?;
/// # let _ = leaf;
/// # Ok(())
/// # }
/// ```
pub async fn serve<F>(
    listener: TcpListener,
    config: Arc<rustls::ServerConfig>,
    router: axum::Router,
    shutdown: F,
) -> Result<(), Error>
where
    F: core::future::Future<Output = ()> + Send + 'static,
{
    let mut config = Arc::unwrap_or_clone(config);
    // Both, in preference order. A client that offers neither gets a plain HTTP/1.1
    // connection, which still works.
    config.alpn_protocols = alloc::vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));

    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    let mut shutdown = Box::pin(shutdown);
    loop {
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => accepted.map_err(Error::Io)?,
            () = &mut shutdown => return Ok(()),
        };
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            // Full: close at once rather than queue. A legitimate client retries.
            drop(stream);
            continue;
        };
        let acceptor = acceptor.clone();
        let router = router.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let Ok(Ok(stream)) =
                tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await
            else {
                // A failed or stalled handshake is a port scan or a wrong pin; not an
                // event.
                return;
            };
            // Attach the peer address the way `axum::serve`'s
            // `into_make_service_with_connect_info` does. Without it there is no
            // `ConnectInfo` for the same-subnet check in `lan_router` to read, and every
            // LAN-only request fails closed — safe, but silently so: the endpoint would
            // look broken rather than protected.
            let service = hyper_util::service::TowerToHyperService::new(
                tower::ServiceBuilder::new()
                    .map_request(move |mut request: axum::http::Request<_>| {
                        request
                            .extensions_mut()
                            .insert(axum::extract::ConnectInfo(peer));
                        request
                    })
                    .service(router),
            );
            let mut builder =
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
            builder
                .http1()
                .timer(hyper_util::rt::TokioTimer::new())
                .header_read_timeout(HANDSHAKE_TIMEOUT);
            let _ = builder
                .serve_connection_with_upgrades(hyper_util::rt::TokioIo::new(stream), service)
                .await;
        });
    }
}
