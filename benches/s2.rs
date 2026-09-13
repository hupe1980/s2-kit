//! What a gateway actually spends its time on.
//!
//! An S2 gateway does three things per message: decode it, validate it, and answer it.
//! A home battery sends an `FRBC.StorageStatus` every few seconds and a hub may carry a
//! thousand of them, so the numbers here are the ones that decide whether that hub is a
//! Raspberry Pi or a server.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};

use s2_kit::codec::{self, DecodeOptions};
use s2_kit::message::Message;
use s2_kit::session::{RmConfig, RmSession};
use s2_kit::testing;
use s2_kit::types::Timestamp;
use s2_kit::validate::{Context, Validate};

/// A small message: the one a battery sends constantly.
const STATUS: &str = r#"{"message_type":"FRBC.StorageStatus","message_id":"b6f3b0d6-3a5e-4d3a-9f2c-1f0a1b2c3d4e","present_fill_level":62.5}"#;

/// A large one: a system description with several actuators and operation modes, which is
/// what a device sends once and a CEM has to understand entirely.
fn big() -> String {
    codec::encode(&Message::from(testing::battery_system(
        Timestamp::UNIX_EPOCH,
    )))
}

fn decoding(c: &mut Criterion) {
    let large = big();
    let mut group = c.benchmark_group("decode");

    group.throughput(Throughput::Bytes(STATUS.len() as u64));
    group.bench_function("small/strict", |b| {
        b.iter(|| codec::decode(std::hint::black_box(STATUS)).unwrap());
    });

    group.throughput(Throughput::Bytes(large.len() as u64));
    group.bench_function("large/strict", |b| {
        b.iter(|| codec::decode(std::hint::black_box(large.as_str())).unwrap());
    });

    // Lenient decoding walks the schema table to prune, so it is the interesting one for
    // a proxy.
    let lenient = DecodeOptions::default().lenient();
    group.bench_function("large/lenient", |b| {
        b.iter(|| codec::decode_with(std::hint::black_box(large.as_str()), &lenient).unwrap());
    });

    // Peeking should be far cheaper than decoding: a router that only needs the message
    // type must not pay for the whole model.
    group.bench_function("large/peek", |b| {
        b.iter(|| codec::peek(std::hint::black_box(large.as_str())).unwrap());
    });
    group.finish();
}

fn encoding(c: &mut Criterion) {
    let small = codec::decode(STATUS).unwrap();
    let large = codec::decode(&big()).unwrap();
    let mut group = c.benchmark_group("encode");
    group.bench_function("small", |b| {
        b.iter(|| codec::encode(std::hint::black_box(&small)));
    });
    group.bench_function("large", |b| {
        b.iter(|| codec::encode(std::hint::black_box(&large)));
    });
    group.finish();
}

fn validating(c: &mut Criterion) {
    let small = codec::decode(STATUS).unwrap();
    let large = codec::decode(&big()).unwrap();
    let ctx = Context::empty();
    let mut group = c.benchmark_group("validate");
    group.bench_function("small", |b| {
        b.iter(|| std::hint::black_box(&small).validate(&ctx));
    });
    group.bench_function("large", |b| {
        b.iter(|| std::hint::black_box(&large).validate(&ctx));
    });
    group.finish();
}

fn sessions(c: &mut Criterion) {
    let mut group = c.benchmark_group("session");

    // The whole path a real message takes: bytes in, validated, registered, answered.
    group.bench_function("handle_text/status", |b| {
        b.iter_batched(
            || {
                let mut rm = RmSession::new(RmConfig::default(), testing::battery_details());
                rm.open(Timestamp::UNIX_EPOCH);
                // Drain the handshake so we measure the steady state, not the opening.
                while rm.poll_transmit().is_some() {}
                rm
            },
            |mut rm| {
                let _ = rm.handle_text(std::hint::black_box(STATUS), Timestamp::UNIX_EPOCH);
                while rm.poll_transmit().is_some() {}
            },
            BatchSize::SmallInput,
        );
    });

    // A full handshake, which is what a fleet manager pays per reconnecting device.
    group.bench_function("handshake", |b| {
        b.iter(|| {
            let mut conversation = testing::Conversation::battery();
            conversation.open();
            std::hint::black_box(conversation.transcript().len())
        });
    });
    group.finish();
}

criterion_group!(benches, decoding, encoding, validating, sessions);
criterion_main!(benches);
