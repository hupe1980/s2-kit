//! Serving the router over TLS.
//!
//! `axum` has no opinion about TLS and neither does this crate, beyond the one thing S2
//! Connect makes non-negotiable: the certificate a LAN endpoint serves is the one clients
//! bind their challenge responses to, so it cannot be changed while an endpoint is up
//! without breaking every pairing in flight.

use alloc::sync::Arc;

use tokio::net::TcpListener;

use super::tls_error::Error;

/// Serve a router over TLS until the future returned by `shutdown` completes.
///
/// One task per connection, `http1` and `h2` both negotiated through ALPN. A handshake
/// that fails is dropped silently: that is a port scan, a browser, or a client with the
/// wrong pin, and none of them is worth stopping the endpoint for.
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

    let mut shutdown = Box::pin(shutdown);
    loop {
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => accepted.map_err(Error::Io)?,
            () = &mut shutdown => return Ok(()),
        };
        let acceptor = acceptor.clone();
        let router = router.clone();
        tokio::spawn(async move {
            let Ok(stream) = acceptor.accept(stream).await else {
                // A failed handshake is a port scan or a wrong pin; not an event.
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
            let _ =
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                    .serve_connection_with_upgrades(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
        });
    }
}
