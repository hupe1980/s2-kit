//! What an operator sees in the logs, with the `tracing` feature on.
//!
//! A binary of its own on purpose: `tracing` caches whether a call site is interesting
//! process-wide, so a scoped subscriber in one test can lose events to call sites another
//! test thread reached first with no subscriber at all. A process with one test has no
//! other thread to race.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use s2_kit::io::{Driver, Error, TextTransport};
use s2_kit::prelude::*;

/// A transport that plays a script and then waits for ever.
struct Scripted(VecDeque<String>);

impl TextTransport for Scripted {
    async fn send_text(&mut self, _text: String) -> Result<(), Error> {
        tokio::task::yield_now().await;
        Ok(())
    }
    async fn recv_text(&mut self) -> Result<Option<String>, Error> {
        if let Some(text) = self.0.pop_front() {
            return Ok(Some(text));
        }
        std::future::pending::<()>().await;
        unreachable!()
    }
    async fn ping(&mut self) -> Result<(), Error> {
        tokio::task::yield_now().await;
        Ok(())
    }
    async fn close(&mut self) {}
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

fn clock() -> Timestamp {
    "2024-01-01T12:00:00Z".parse().unwrap()
}

#[tokio::test]
async fn a_refusal_is_traceable_to_its_exchange_and_never_logs_the_payload() {
    // An operator who sees a refusal in a fleet's logs must be
    // able to follow it to the message and the session — by `message_id`, the key the
    // `ReceptionStatus` carries back — without the log ever holding the body.
    let captured = Captured::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_writer(captured.clone())
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .finish(),
    )
    .unwrap();

    let mut rm = RmSession::new(RmConfig::default(), s2_kit::testing::battery_details());
    rm.open(clock());
    let transport = Scripted(VecDeque::from([
        r#"{"message_type":"Handshake","message_id":"c1","role":"CEM","supported_protocol_versions":["1.0.0"]}"#.to_string(),
        r#"{"message_type":"HandshakeResponse","message_id":"c2","selected_protocol_version":"1.0.0"}"#.to_string(),
        // A measurement from the CEM: well-formed, and wrong for the role, so refused.
        r#"{"message_type":"PowerMeasurement","message_id":"secret-77","measurement_timestamp":"2024-01-01T12:00:00Z","values":[{"commodity_quantity":"ELECTRIC.POWER.L1","value":4242.5}]}"#.to_string(),
    ]));
    let mut driver = Driver::new(transport)
        .with_clock(clock)
        .with_span(tracing::info_span!(
            "s2.session",
            resource = "battery-1",
            state = tracing::field::Empty,
            version = tracing::field::Empty,
        ));
    for _ in 0..3 {
        driver.step(&mut rm).await.unwrap();
    }

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let refusal = log
        .lines()
        .find(|l| l.contains("refused an inbound message"))
        .unwrap_or_else(|| panic!("no refusal logged:\n{log}"));
    assert!(refusal.contains("secret-77"), "{refusal}");
    assert!(refusal.contains("resource=\"battery-1\""), "{refusal}");
    assert!(
        refusal.contains("S2-"),
        "the rule id makes it greppable: {refusal}"
    );
    assert!(log.contains("version=\"1.0.0\""), "{log}");
    assert!(
        !log.contains("4242.5"),
        "a payload value reached the log:\n{log}"
    );
}
