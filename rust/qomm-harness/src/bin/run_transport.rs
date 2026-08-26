//! Rust port of `scripts/run_transport.py`.

use qomm_audit::receipts::digest;
use qomm_harness::measure::{render, scaled, summarise};
use qomm_harness::{next_value, parse_value, write_pretty_json, HarnessResult};
use qomm_sim::attackers::auc;
use qomm_sim::market::py_round;
use qomm_sim::pyrandom::PyRandom;
use qomm_transport::client::{Client, N_REQUEST_VALUES};
use qomm_transport::relay::{NodeInbox, Relay};
use qomm_transport::wire::{reconstruct, FRAME_BYTES};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

struct Options {
    out: PathBuf,
    clients: usize,
    nodes: usize,
    slots: u32,
    slot_ms: f64,
    activity: f64,
    seed: u64,
    hops: Vec<usize>,
    link_ms: f64,
}

struct Session {
    clients: Vec<Client>,
    inboxes: Vec<Arc<Mutex<NodeInbox>>>,
    truth: BTreeMap<(usize, u32), bool>,
    slot_wall: Vec<f64>,
    phases: Vec<f64>,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    let mut by_hops = Vec::new();
    for &hops in &options.hops {
        println!("== relay cascade of {hops} hop(s) ==");
        let session = run_session(&options, hops)?;
        let mut report = analyse(&session, &options)?;
        report["hops"] = json!(hops);
        report["link_ms"] = json!(options.link_ms);
        report["slot_phases_ms"] = Value::Array(
            session
                .phases
                .iter()
                .map(|value| json!(py_round(value * 1_000.0) as f64 / 1_000.0))
                .collect(),
        );
        report["added_latency_ms"] = json!(qomm_sim::fsum::nsum(
            (1..hops).map(|hop| options.link_ms + session.phases[hop])
        ));
        let mut ordered = session.slot_wall.clone();
        ordered.sort_by(f64::total_cmp);
        report["slot_wall_median_ms"] = json!(1e3 * ordered[ordered.len() / 2]);
        print_report(&report);
        by_hops.push(report);
    }

    let payload = json!({
        "host": qomm_measure::hosts::this_host(),
        "config": {
            "clients": options.clients,
            "nodes": options.nodes,
            "slots": options.slots,
            "slot_ms": options.slot_ms,
            "activity": options.activity,
            "seed": options.seed,
            "hops": options.hops,
            "link_ms": options.link_ms,
        },
        "by_hops": by_hops,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn run_session(options: &Options, hops: usize) -> HarnessResult<Session> {
    if hops == 0 {
        return Err("--hops values must be positive".into());
    }
    let mut rng = PyRandom::new(options.seed);
    let mut phase_rng = PyRandom::new(options.seed + 7_717);
    let phases = (0..hops)
        .map(|_| phase_rng.uniform(0.0, options.slot_ms))
        .collect::<Vec<_>>();
    let inboxes = (0..options.nodes)
        .map(|node| Arc::new(Mutex::new(NodeInbox::new(node as u16))))
        .collect::<Vec<_>>();

    let mut cascades = Vec::with_capacity(options.nodes);
    for (node, inbox) in inboxes.iter().enumerate() {
        let mut reversed = Vec::with_capacity(hops);
        let mut downstream_port = None;
        for hop in (0..hops).rev() {
            let target = (hop == hops - 1).then(|| Arc::clone(inbox));
            let mut relay = Relay::new(node as u16, target, downstream_port, hop, None);
            downstream_port = Some(relay.start()?);
            reversed.push(relay);
        }
        reversed.reverse();
        cascades.push(reversed);
    }
    let ports: Vec<u16> = cascades.iter().map(|chain| chain[0].port()).collect();
    let mut clients = (0..options.clients)
        .map(|client_id| {
            let client_bytes = u16::try_from(client_id)
                .map(u16::to_be_bytes)
                .map_err(|_| "client index is outside the Python two-byte key domain")?;
            Ok(Client::new(
                client_id,
                digest(&[b"client", &client_bytes]).to_vec(),
                ports.clone(),
            ))
        })
        .collect::<HarnessResult<Vec<_>>>()?;
    for client in &mut clients {
        client.connect()?;
    }

    let mut truth = BTreeMap::new();
    let mut slot_wall = Vec::with_capacity(options.slots as usize);
    for slot in 0..options.slots {
        let started = Instant::now();
        for client in &mut clients {
            let active = rng.random() < options.activity;
            truth.insert((client.client_id, slot), active);
            let request = active.then(|| {
                [
                    1,
                    rng.randint(1, 400) as u128,
                    rng.randrange(0, 2) as u128,
                    client.client_id as u128,
                ]
            });
            client.send_slot(slot, request).map_err(|error| {
                format!(
                    "client {} could not send slot {slot}: {error}",
                    client.client_id
                )
            })?;
        }
        thread::sleep(Duration::from_secs_f64(options.slot_ms / 1_000.0));
        for hop in 0..hops {
            for chain in &cascades {
                chain[hop].close_slot(slot).map_err(|error| {
                    format!("relay hop {hop} could not close slot {slot}: {error}")
                })?;
            }
            if hop + 1 < hops {
                thread::sleep(Duration::from_secs_f64(options.link_ms / 1_000.0));
                thread::sleep(Duration::from_secs_f64(phases[hop + 1] / 1_000.0));
            } else {
                thread::sleep(Duration::from_millis(2));
            }
        }
        slot_wall.push(started.elapsed().as_secs_f64());
    }

    for client in &mut clients {
        client.close();
    }
    thread::sleep(Duration::from_millis(50));
    for chain in &mut cascades {
        for relay in chain {
            relay.stop();
        }
    }
    Ok(Session {
        clients,
        inboxes,
        truth,
        slot_wall,
        phases,
    })
}

fn analyse(session: &Session, options: &Options) -> HarnessResult<Value> {
    let mut per_client_slot: BTreeMap<(usize, u32), Vec<usize>> = BTreeMap::new();
    for client in &session.clients {
        for record in &client.sends {
            per_client_slot
                .entry((client.client_id, record.slot))
                .or_default()
                .push(record.size);
        }
    }
    let frame_counts = per_client_slot
        .values()
        .map(Vec::len)
        .collect::<BTreeSet<_>>();
    let byte_totals = per_client_slot
        .values()
        .map(|sizes| sizes.iter().sum::<usize>())
        .collect::<BTreeSet<_>>();
    let active_bytes = per_client_slot
        .iter()
        .filter(|(key, _)| session.truth[key])
        .map(|(_, sizes)| sizes.iter().sum::<usize>())
        .collect::<BTreeSet<_>>();
    let idle_bytes = per_client_slot
        .iter()
        .filter(|(key, _)| !session.truth[key])
        .map(|(_, sizes)| sizes.iter().sum::<usize>())
        .collect::<BTreeSet<_>>();

    let mut delivered = 0usize;
    let mut batch_sizes = BTreeSet::new();
    let mut scores = Vec::with_capacity(options.clients * options.slots as usize);
    let mut labels = Vec::with_capacity(scores.capacity());
    for slot in 0..options.slots {
        let mut positions: BTreeMap<usize, usize> = BTreeMap::new();
        for inbox in &session.inboxes {
            let inbox = inbox.lock().expect("relay inbox lock");
            let batch = inbox.frames.get(&slot).map(Vec::as_slice).unwrap_or(&[]);
            delivered += batch.len();
            batch_sizes.insert(batch.len());
            for order in 0..batch.len() {
                *positions.entry(order).or_default() += 1;
            }
        }
        for client in 0..options.clients {
            scores.push(positions.len() as f64);
            labels.push(u8::from(session.truth[&(client, slot)]));
        }
    }
    let linkage_auc = auc(&scores, &labels);
    let base_rate = if labels.is_empty() {
        0.0
    } else {
        labels
            .iter()
            .map(|value| usize::from(*value))
            .sum::<usize>() as f64
            / labels.len() as f64
    };

    let single_relay_recovers = session
        .inboxes
        .first()
        .and_then(|inbox| {
            inbox
                .lock()
                .expect("relay inbox lock")
                .frames
                .get(&0)
                .and_then(|frames| frames.first().cloned())
        })
        .map(|frame| reconstruct(&[frame.payload], N_REQUEST_VALUES))
        .transpose()?
        .is_some_and(|values| {
            values.iter().any(|value| {
                value
                    .as_u128()
                    .is_some_and(|value| value > 0 && value < 10_000)
            })
        });

    Ok(json!({
        "frames_per_client_slot": frame_counts,
        "bytes_per_client_slot": byte_totals,
        "active_client_bytes": active_bytes,
        "idle_client_bytes": idle_bytes,
        "traffic_identical": byte_totals.len() == 1 && active_bytes == idle_bytes,
        "frame_bytes": FRAME_BYTES,
        "batch_sizes_at_node": batch_sizes,
        "frames_delivered": delivered,
        "expected_frames": options.clients * options.nodes * options.slots as usize,
        "linkage_auc": linkage_auc,
        "linkage_base_rate": base_rate,
        "linkage_advantage": linkage_auc.map(|value| (value - 0.5).abs() * 2.0),
        "single_relay_recovers_request": single_relay_recovers,
        "slot_wall_s": summarise(&session.slot_wall),
    }))
}

fn print_report(report: &Value) {
    println!(
        "  hops / link one-way        : {} / {} ms",
        report["hops"], report["link_ms"]
    );
    println!(
        "  relay boundary offsets     : {}",
        qomm_harness::py_display(&report["slot_phases_ms"])
    );
    println!(
        "  added latency vs one hop   : {:.1} ms",
        report["added_latency_ms"].as_f64().unwrap_or(0.0)
    );
    println!(
        "  slot wall clock (median)   : {:.1} ms",
        report["slot_wall_median_ms"].as_f64().unwrap_or(0.0)
    );
    println!("  frame size                 : {} B", report["frame_bytes"]);
    println!(
        "  frames per client per slot : {}",
        qomm_harness::py_display(&report["frames_per_client_slot"])
    );
    println!(
        "  bytes per client per slot  : {}",
        qomm_harness::py_display(&report["bytes_per_client_slot"])
    );
    println!(
        "  active vs idle bytes       : {} vs {}",
        qomm_harness::py_display(&report["active_client_bytes"]),
        qomm_harness::py_display(&report["idle_client_bytes"])
    );
    println!(
        "  traffic identical          : {}",
        report["traffic_identical"]
    );
    println!(
        "  frames delivered / expected: {} / {}",
        report["frames_delivered"], report["expected_frames"]
    );
    println!(
        "  batch sizes seen at a node : {}",
        qomm_harness::py_display(&report["batch_sizes_at_node"])
    );
    println!(
        "  origin-linkage AUC         : {:.3} (base rate {:.3}, advantage {:.3})",
        report["linkage_auc"].as_f64().unwrap_or(f64::NAN),
        report["linkage_base_rate"].as_f64().unwrap_or(0.0),
        report["linkage_advantage"].as_f64().unwrap_or(f64::NAN),
    );
    println!(
        "  one relay recovers request : {}",
        report["single_relay_recovers_request"]
    );
    println!(
        "  slot wall clock            : {}",
        render(&scaled(&report["slot_wall_s"], 1e3), 1, " ms")
    );
}

fn parse_args() -> HarnessResult<Options> {
    let mut out = None;
    let mut clients: usize = 12;
    let mut nodes: usize = 7;
    let mut slots: u32 = 40;
    let mut slot_ms: f64 = 25.0;
    let mut activity: f64 = 0.3;
    let mut seed: u64 = 11;
    let mut hops: Vec<usize> = vec![1, 2, 3];
    let mut link_ms: f64 = 0.0;
    let mut args = std::env::args_os().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--out") => out = Some(PathBuf::from(next_value(&mut args, "--out")?)),
            Some("--clients") => {
                clients = parse_value(next_value(&mut args, "--clients")?, "--clients")?
            }
            Some("--nodes") => nodes = parse_value(next_value(&mut args, "--nodes")?, "--nodes")?,
            Some("--slots") => slots = parse_value(next_value(&mut args, "--slots")?, "--slots")?,
            Some("--slot-ms") => {
                slot_ms = parse_value(next_value(&mut args, "--slot-ms")?, "--slot-ms")?
            }
            Some("--activity") => {
                activity = parse_value(next_value(&mut args, "--activity")?, "--activity")?
            }
            Some("--seed") => seed = parse_value(next_value(&mut args, "--seed")?, "--seed")?,
            Some("--link-ms") => {
                link_ms = parse_value(next_value(&mut args, "--link-ms")?, "--link-ms")?
            }
            Some("--hops") => {
                let mut parsed = Vec::new();
                while args
                    .peek()
                    .is_some_and(|value| !value.to_string_lossy().starts_with("--"))
                {
                    parsed.push(parse_value(args.next().expect("peeked value"), "--hops")?);
                }
                if parsed.is_empty() {
                    return Err("argument --hops expects at least one value".into());
                }
                hops = parsed;
            }
            Some("-h" | "--help") => {
                println!("usage: run_transport --out PATH [--clients N] [--nodes N] [--slots N] [--slot-ms MS] [--activity P] [--seed N] [--hops N ...] [--link-ms MS]");
                std::process::exit(0);
            }
            _ => {
                return Err(
                    format!("unknown argument {}", OsString::from(arg).to_string_lossy()).into(),
                )
            }
        }
    }
    let out = out.ok_or("--out is required")?;
    if clients == 0 || nodes < 2 || slots == 0 {
        return Err("--clients and --slots must be positive; --nodes must be at least two".into());
    }
    if !slot_ms.is_finite() || slot_ms < 0.0 || !link_ms.is_finite() || link_ms < 0.0 {
        return Err("slot and link durations must be finite and non-negative".into());
    }
    if !activity.is_finite() || !(0.0..=1.0).contains(&activity) {
        return Err("--activity must be between zero and one".into());
    }
    Ok(Options {
        out,
        clients,
        nodes,
        slots,
        slot_ms,
        activity,
        seed,
        hops,
        link_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These three run a wall-clock slot schedule over real loopback sockets, so
    /// two of them at once contend for the CPU and frames miss their slot. That
    /// is not a transport fault and it is not a build-profile one either: the
    /// release binary delivers 144 of 144 frames eight runs out of eight, while
    /// `cargo test --release` fails three runs out of five and
    /// `--test-threads=1` passes six out of six.
    ///
    /// The Python they were ported from never met this, because pytest runs a
    /// module's tests one after another and `cargo test` runs them in parallel
    /// threads. A suite that measures time does not survive that change of
    /// runner by itself, and no test count would have shown it.
    fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
        static SLOT_CLOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        SLOT_CLOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn options(link_ms: f64) -> Options {
        Options {
            out: PathBuf::new(),
            clients: 6,
            nodes: 3,
            slots: 4,
            slot_ms: 10.0,
            activity: 0.4,
            seed: 3,
            hops: vec![],
            link_ms,
        }
    }

    fn median(values: &[f64]) -> f64 {
        let mut values = values.to_vec();
        values.sort_by(f64::total_cmp);
        values[values.len() / 2]
    }

    /// `tests/test_gaps.py::test_every_relay_hop_costs_a_connection`, which runs
    /// eight slots and no link delay. It asserts an exact delivery count, so the
    /// delay has to stay out of it: three hops at 5 ms against a 10 ms slot is a
    /// race, and it loses --- 67 of 72 frames arrived on the measurement host
    /// while all 72 arrived on the laptop. What the delay is for is
    /// `a_link_delay_between_relays_is_actually_paid`, which measures time
    /// rather than counting frames.
    #[test]
    fn every_relay_hop_costs_a_connection() {
        let _serial = one_at_a_time();
        let mut options = options(0.0);
        options.slots = 8;
        let one = run_session(&options, 1).unwrap();
        let three = run_session(&options, 3).unwrap();
        for session in [&one, &three] {
            let report = analyse(session, &options).unwrap();
            assert_eq!(report["frames_delivered"], report["expected_frames"]);
            assert_eq!(report["traffic_identical"], true);
        }
        assert!(median(&three.slot_wall) > median(&one.slot_wall));
    }

    #[test]
    fn relays_do_not_share_a_slot_boundary() {
        let _serial = one_at_a_time();
        let mut options = options(5.0);
        options.clients = 3;
        options.slots = 2;
        options.slot_ms = 25.0;
        options.seed = 5;
        let session = run_session(&options, 3).unwrap();
        assert_eq!(session.phases.len(), 3);
        assert!(
            session
                .phases
                .iter()
                .map(|phase| (phase * 1_000_000.0).round() as i64)
                .collect::<BTreeSet<_>>()
                .len()
                > 1
        );
        assert!(session
            .phases
            .iter()
            .all(|phase| (0.0..=25.0).contains(phase)));
    }

    #[test]
    fn a_link_delay_between_relays_is_actually_paid() {
        let _serial = one_at_a_time();
        let mut without_delay = options(0.0);
        without_delay.clients = 3;
        without_delay.slots = 3;
        without_delay.slot_ms = 25.0;
        without_delay.seed = 5;
        let mut with_delay = options(12.0);
        with_delay.clients = 3;
        with_delay.slots = 3;
        with_delay.slot_ms = 25.0;
        with_delay.seed = 5;
        let baseline = run_session(&without_delay, 3).unwrap();
        let delayed = run_session(&with_delay, 3).unwrap();
        assert!(median(&delayed.slot_wall) > median(&baseline.slot_wall));
    }
}
