//!
//! One long-lived OS process represents each quorum member.  Public VSS ladders
//! use a shared mailbox, but recipient evaluations are encrypted to an
//! ephemeral key that exists only inside that recipient process.  Every child
//! also attempts to decrypt all foreign mailbox records and refuses to proceed
//! if any succeeds.  The parent sees only public keys, Pedersen-VSS ladders,
//! first moves, the public challenge, and partial responses.

use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::Identity;
use ed25519_dalek::{Signature, SigningKey};
use merlin::Transcript;
use qomm_harness::{
    median, parse_value, repo_root, unique_temp_dir, write_pretty_json, HarnessResult,
};
use rand::rngs::OsRng;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use zkfmi_crypto::{
    backend::MlDsa65Signer,
    key::{KeyId, KeyPurpose, KeyRecord, ParticipantId},
    traits::Signer as _,
};
use zkfmi_zk::pedersen::Pedersen;
use zkfmi_zk::shamir;
use zkfmi_zk::sigma::{product_challenge, verify_product};
use zkpi_committee::selective_disclosure::{
    open_if_winner, seal_for_winner, WinnerEnvelope, WinnerPrivateKey, WinnerPublicKey,
    WinnerSenderAuth, AUTH_SUITE, KEM_SUITE, VERSION,
};
use zkpi_proofs::threshold_gadgets::{
    audit_recorded_product_partials, coefficient_commitments_from_evaluations,
    CommittedContributions, DealerCoefficientCommitments, ProductAssemblyTranscript,
};
use zkpi_proofs::threshold_sigma::{combine_commitments, share_commitment, PartyId};

const PRIVATE_SLOTS: usize = 5;

struct Options {
    out: PathBuf,
    parties: usize,
    threshold: usize,
    repeats: usize,
    group: String,
}

struct NodeProcess {
    party: PartyId,
    child: Child,
    input: ChildStdin,
    output: BufReader<std::process::ChildStdout>,
}

struct Mailbox(PathBuf);

impl Mailbox {
    fn new() -> HarnessResult<Self> {
        Ok(Self(unique_temp_dir("qomm-distributed")?))
    }
}

impl Drop for Mailbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn main() {
    let mut args = std::env::args_os();
    let _ = args.next();
    let result = if args.next().as_deref() == Some(std::ffi::OsStr::new("__node")) {
        node_main(args.collect())
    } else {
        run_main()
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    if options.group != "ed25519" {
        return Err("the Rust proof crates implement --group ed25519 as ristretto255; no other group is available".into());
    }
    if options.repeats == 0 {
        return Err("--repeats must be positive".into());
    }
    if options.parties <= options.threshold {
        return Err("--parties must be greater than --threshold".into());
    }

    let key = Pedersen::new(b"qomm:quote:v1");
    let quorum = (1..=options.threshold + 1).collect::<Vec<_>>();
    let executable = std::env::current_exe()?;
    let mut wall_samples = Vec::new();
    let mut wire_bytes = Vec::new();
    let mut node_waits = Vec::new();

    for _ in 0..options.repeats {
        let mailbox = Mailbox::new()?;
        let started = Instant::now();
        let mut nodes = quorum
            .iter()
            .map(|party| spawn_node(&executable, *party, &mailbox.0, &quorum, options.threshold))
            .collect::<HarnessResult<Vec<_>>>()?;

        let mut recipient_keys = BTreeMap::new();
        for node in &mut nodes {
            let line = read_line(&mut node.output, "recipient encryption key")?;
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() != 3 || fields[0] != "KEY" {
                return Err(format!(
                    "node {} returned malformed recipient key: {line}",
                    node.party
                )
                .into());
            }
            let party = fields[1].parse::<PartyId>()?;
            if party != node.party
                || recipient_keys
                    .insert(party, decode_key(fields[2])?)
                    .is_some()
            {
                return Err(format!("invalid or duplicate recipient key for party {party}").into());
            }
        }
        let encoded_keys = recipient_keys
            .iter()
            .map(|(party, key)| format!("{party}:{}", hex::encode(key)))
            .collect::<Vec<_>>()
            .join(",");
        for node in &mut nodes {
            writeln!(node.input, "KEYS {encoded_keys}")?;
            node.input.flush()?;
        }
        for node in &mut nodes {
            let line = read_line(&mut node.output, "dealer seal")?;
            if line != format!("SEALED {}", node.party) {
                return Err(
                    format!("node {} did not seal before opening: {line}", node.party).into(),
                );
            }
        }
        // No encrypted delivery is written until every dealer has fixed its
        // public coefficient commitments.
        for node in &mut nodes {
            writeln!(node.input, "OPEN")?;
            node.input.flush()?;
        }

        let mut c_a = None;
        let mut factor_ladder = None;
        let mut factor_parts = BTreeMap::new();
        let mut product_parts = BTreeMap::new();
        let mut relation_evaluations = BTreeMap::new();
        let mut nonce_seals: DealerCoefficientCommitments = BTreeMap::new();
        let mut held_counts = Vec::new();
        let mut sent = 0u64;

        for node in &mut nodes {
            let line = read_line(&mut node.output, "node first move")?;
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() != 13 || fields[0] != "READY" {
                return Err(
                    format!("node {} returned malformed first move: {line}", node.party).into(),
                );
            }
            let party = fields[1].parse::<usize>()?;
            if party != node.party {
                return Err(format!("node {} answered as party {party}", node.party).into());
            }
            let node_c_a = decode_point(fields[2])?;
            if c_a
                .as_ref()
                .is_some_and(|point: &RistrettoPoint| point.compress() != node_c_a.compress())
            {
                return Err("nodes derived different aggregate commitments".into());
            }
            c_a = Some(node_c_a);
            factor_parts.insert(party, decode_point(fields[3])?);
            product_parts.insert(party, decode_point(fields[4])?);
            relation_evaluations.insert(party, decode_point(fields[5])?);
            sent += fields[6].parse::<u64>()?;
            held_counts.push(fields[7].parse::<usize>()?);
            let node_factor_ladder = decode_ladder(fields[8])?;
            if factor_ladder
                .as_ref()
                .is_some_and(|ladder: &Vec<RistrettoPoint>| {
                    !same_ladder(ladder, &node_factor_ladder)
                })
            {
                return Err("nodes derived different factor VSS ladders".into());
            }
            factor_ladder = Some(node_factor_ladder);
            nonce_seals.insert(
                party,
                vec![
                    decode_ladder(fields[9])?,
                    decode_ladder(fields[10])?,
                    decode_ladder(fields[11])?,
                ],
            );
            if fields[12] != "foreign_decryptions=0" {
                return Err(format!(
                    "node {party} could read another child's private material: {}",
                    fields[12]
                )
                .into());
            }
        }
        if held_counts.iter().any(|count| *count != 3) {
            return Err(format!(
                "a node was handed {} witness values, not one share of each",
                held_counts.iter().copied().max().unwrap_or(0)
            )
            .into());
        }

        let c_a = c_a.ok_or("the quorum published no aggregate commitment")?;
        let factor_ladder = factor_ladder.ok_or("the quorum published no factor ladder")?;
        if factor_ladder.len() != options.threshold + 1
            || factor_ladder[0].compress() != c_a.compress()
        {
            return Err("the factor VSS ladder does not open at the aggregate commitment".into());
        }
        let cross_ladder =
            coefficient_commitments_from_evaluations(&relation_evaluations, options.threshold)?;
        if cross_ladder[0].compress() != c_a.compress() {
            return Err("the distributed product relation is not committed to the product".into());
        }
        let t_factor = combine_commitments(&factor_parts)?;
        let t_product = combine_commitments(&product_parts)?;
        let mut transcript = tagged();
        let challenge = product_challenge(&mut transcript, &c_a, &c_a, &c_a, &t_factor, &t_product);
        for node in &mut nodes {
            writeln!(node.input, "{}", encode_scalar(&challenge))?;
            node.input.flush()?;
        }

        let mut answers = BTreeMap::new();
        let mut waits = Vec::new();
        for node in &mut nodes {
            let line = read_line(&mut node.output, "node answer")?;
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() != 7 || fields[0] != "ANSWER" {
                return Err(
                    format!("node {} returned malformed answer: {line}", node.party).into(),
                );
            }
            let party = fields[1].parse::<usize>()?;
            if party != node.party {
                return Err(format!("node {} answered as party {party}", node.party).into());
            }
            answers.insert(
                party,
                (
                    decode_scalar(fields[2])?,
                    decode_scalar(fields[3])?,
                    decode_scalar(fields[4])?,
                ),
            );
            waits.push(fields[5].parse::<f64>()? * 1_000.0);
            if fields[6] != "done" {
                return Err(format!("node {party} did not complete its response").into());
            }
        }
        for mut node in nodes {
            let status = node.child.wait()?;
            if !status.success() {
                return Err(format!("node {} exited {status}", node.party).into());
            }
        }

        let record = ProductAssemblyTranscript {
            quorum: quorum.clone(),
            challenge,
            c_a,
            factor_parts,
            product_parts,
            answers,
            share_coefficient_commitments: factor_ladder,
            cross_coefficient_commitments: cross_ladder,
            nonce_seals,
        };
        if !audit_recorded_product_partials(&key, &record).is_empty() {
            return Err("a distributed partial response failed its Pedersen-VSS audit".into());
        }
        let proof = record.assemble()?;
        let mut transcript = tagged();
        if !verify_product(&key, &mut transcript, &c_a, &c_a, &c_a, &proof) {
            return Err("the distributed assembly did not verify".into());
        }

        wall_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        wire_bytes.push(sent + 32 * quorum.len() as u64);
        node_waits.push(median(&waits).expect("one process per quorum member"));
    }

    let payload = json!({
        "host": zkfmi_measure::hosts::this_host(),
        "group": options.group,
        "parties": options.parties,
        "threshold": options.threshold,
        "processes": quorum.len(),
        "each_node_held": "one share of the value, its blinding and the cross term, and nothing of any other node's",
        "filesystem_foreign_private_decryptions": 0,
        "private_transport": "recipient-key encrypted mailbox records; recipient secret exists only in child memory",
        "wall_ms": {
            "median": median(&wall_samples),
            "n": wall_samples.len(),
            "min": wall_samples.iter().copied().min_by(f64::total_cmp),
            "max": wall_samples.iter().copied().max_by(f64::total_cmp),
        },
        "node_wait_ms": {"median": median(&node_waits)},
        "bytes_between_nodes": {"median": integer_median(&wire_bytes)},
        "verified": true,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!(
        "{} processes, one share each: wall {:.1} ms, node waits {:.1} ms, {:.0} B between them",
        quorum.len(),
        payload["wall_ms"]["median"].as_f64().unwrap(),
        payload["node_wait_ms"]["median"].as_f64().unwrap(),
        payload["bytes_between_nodes"]["median"].as_f64().unwrap(),
    );
    println!("wrote {}", options.out.display());
    Ok(())
}

fn spawn_node(
    executable: &Path,
    party: PartyId,
    mailbox: &Path,
    quorum: &[PartyId],
    threshold: usize,
) -> HarnessResult<NodeProcess> {
    let mut child = Command::new(executable)
        .arg("__node")
        .arg(party.to_string())
        .arg(mailbox)
        .arg(
            quorum
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
        .arg(threshold.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let input = child.stdin.take().ok_or("node stdin was not piped")?;
    let output = BufReader::new(child.stdout.take().ok_or("node stdout was not piped")?);
    Ok(NodeProcess {
        party,
        child,
        input,
        output,
    })
}

fn node_main(args: Vec<OsString>) -> HarnessResult<()> {
    if args.len() != 4 {
        return Err("internal node mode expects party, mailbox, quorum, and threshold".into());
    }
    let party = args[0].to_string_lossy().parse::<usize>()?;
    let mailbox = PathBuf::from(&args[1]);
    let quorum = args[2]
        .to_string_lossy()
        .split(',')
        .map(str::parse)
        .collect::<Result<Vec<usize>, _>>()?;
    let threshold = args[3].to_string_lossy().parse::<usize>()?;
    if !quorum.contains(&party) || quorum.len() != threshold + 1 {
        return Err("node configuration is not one threshold quorum".into());
    }

    let key = Pedersen::new(b"qomm:quote:v1");
    let started = Instant::now();
    let recipient_secret = WinnerPrivateKey::generate()?;
    let recipient_public = recipient_secret.public_key()?.raw_public_key()?;
    println!("KEY {party} {}", hex::encode(&recipient_public));
    std::io::stdout().flush()?;

    let mut key_line = String::new();
    std::io::stdin().read_line(&mut key_line)?;
    let recipient_keys = decode_recipient_keys(key_line.trim(), &quorum)?;
    if recipient_keys[&party].raw_public_key()? != recipient_public {
        return Err("parent changed this node's recipient encryption key".into());
    }

    let dealer_state = prepare_dealer(&key, party, &quorum, threshold)?;
    write_public_deal(&mailbox, party, &dealer_state.public)?;
    println!("SEALED {party}");
    std::io::stdout().flush()?;

    let mut open = String::new();
    std::io::stdin().read_line(&mut open)?;
    if open.trim() != "OPEN" {
        return Err("private delivery was requested before the all-dealer seal barrier".into());
    }
    write_encrypted_deliveries(&mailbox, party, &dealer_state.deliveries, &recipient_keys)?;
    wait_for_dealers(&mailbox, &quorum)?;

    let mut foreign_decryptions = 0usize;
    for dealer in &quorum {
        for recipient in &quorum {
            if *recipient == party {
                continue;
            }
            let encrypted = fs::read_to_string(private_path(&mailbox, *dealer, *recipient))?;
            if decrypt_private(*dealer, *recipient, &recipient_secret, &encrypted).is_ok() {
                foreign_decryptions += 1;
            }
        }
    }
    if foreign_decryptions != 0 {
        return Err(format!(
            "party {party} decrypted {foreign_decryptions} foreign private mailbox records"
        )
        .into());
    }

    let mut factor_value = Scalar::ZERO;
    let mut factor_blinding = Scalar::ZERO;
    let mut cross = Scalar::ZERO;
    let mut nonce = [Scalar::ZERO; 3];
    let mut factor_ladder = vec![RistrettoPoint::identity(); threshold + 1];
    let mut nonce_seals = None;

    for dealer in &quorum {
        let public = read_public(&public_path(&mailbox, *dealer), threshold)?;
        let delivered = read_private(
            &private_path(&mailbox, *dealer, party),
            *dealer,
            party,
            &recipient_secret,
        )?;
        for (slot, ladder) in public.all_ladders().iter().enumerate() {
            let expected = share_commitment(ladder, party)?;
            let actual = key.commit(&delivered[slot].0, &delivered[slot].1);
            if expected.compress() != actual.compress() {
                return Err(format!(
                    "dealer {dealer} sent party {party} an inconsistent slot {slot}"
                )
                .into());
            }
        }
        factor_value += delivered[0].0;
        factor_blinding += delivered[0].1;
        cross += delivered[1].0;
        for slot in 0..3 {
            nonce[slot] += delivered[slot + 2].0;
        }
        for (aggregate, contribution) in factor_ladder.iter_mut().zip(&public.factor) {
            *aggregate += contribution;
        }
        if *dealer == party {
            nonce_seals = Some(public.nonce);
        }
    }
    if factor_value == Scalar::ONE {
        return Err("this randomized run handed a node the clear bit; rerun".into());
    }
    let c_a = factor_ladder[0];
    let expected = share_commitment(&factor_ladder, party)?;
    if key.commit(&factor_value, &factor_blinding).compress() != expected.compress() {
        return Err("aggregate factor share does not match its public VSS ladder".into());
    }
    let factor = key.commit(&nonce[0], &nonce[1]);
    let product = c_a * nonce[0] + key.h * nonce[2];
    let relation = c_a * factor_value + key.h * cross;
    let nonce_seals = nonce_seals.ok_or("node did not recover its public nonce seals")?;
    println!(
        "READY {party} {} {} {} {} {} 3 {} {} {} {} foreign_decryptions={foreign_decryptions}",
        encode_point(&c_a),
        encode_point(&factor),
        encode_point(&product),
        encode_point(&relation),
        factor.compress().as_bytes().len() + product.compress().as_bytes().len(),
        encode_ladder(&factor_ladder),
        encode_ladder(&nonce_seals[0]),
        encode_ladder(&nonce_seals[1]),
        encode_ladder(&nonce_seals[2]),
    );
    std::io::stdout().flush()?;

    let mut challenge = String::new();
    std::io::stdin().read_line(&mut challenge)?;
    let challenge = decode_scalar(challenge.trim())?;
    let answers = [
        nonce[0] + challenge * factor_value,
        nonce[1] + challenge * factor_blinding,
        nonce[2] + challenge * cross,
    ];
    println!(
        "ANSWER {party} {} {} {} {:.9} done",
        encode_scalar(&answers[0]),
        encode_scalar(&answers[1]),
        encode_scalar(&answers[2]),
        started.elapsed().as_secs_f64(),
    );
    std::io::stdout().flush()?;
    Ok(())
}

#[derive(Clone)]
struct PublicDeal {
    factor: Vec<RistrettoPoint>,
    cross: Vec<RistrettoPoint>,
    nonce: Vec<Vec<RistrettoPoint>>,
}

struct DealerState {
    public: PublicDeal,
    deliveries: BTreeMap<PartyId, Vec<(Scalar, Scalar)>>,
}

impl PublicDeal {
    fn all_ladders(&self) -> [&[RistrettoPoint]; PRIVATE_SLOTS] {
        [
            &self.factor,
            &self.cross,
            &self.nonce[0],
            &self.nonce[1],
            &self.nonce[2],
        ]
    }
}

fn prepare_dealer(
    key: &Pedersen,
    dealer: PartyId,
    quorum: &[PartyId],
    threshold: usize,
) -> HarnessResult<DealerState> {
    let mut rng = OsRng;
    let points = quorum
        .iter()
        .map(|party| Scalar::from(*party as u64))
        .collect::<Vec<_>>();
    let constant = if dealer == quorum[0] {
        Scalar::ONE
    } else {
        Scalar::ZERO
    };
    let (factor_values, factor_coefficients) =
        shamir::share_with_coefficients(&constant, threshold, &points, &mut rng);
    let (factor_blindings, factor_blinding_coefficients) =
        shamir::share_with_coefficients(&Scalar::random(&mut rng), threshold, &points, &mut rng);
    let (cross_values, cross_coefficients) =
        shamir::share_with_coefficients(&Scalar::ZERO, threshold, &points, &mut rng);
    let (cross_masks, cross_mask_coefficients) =
        shamir::share_with_coefficients(&Scalar::random(&mut rng), threshold, &points, &mut rng);
    let factor_ladder = factor_coefficients
        .iter()
        .zip(&factor_blinding_coefficients)
        .map(|(value, blinding)| key.commit(value, blinding))
        .collect::<Vec<_>>();
    let cross_ladder = cross_coefficients
        .iter()
        .zip(&cross_mask_coefficients)
        .map(|(value, blinding)| key.commit(value, blinding))
        .collect::<Vec<_>>();

    let contributions = CommittedContributions::new(key, dealer, quorum, threshold, 3, &mut rng)?;
    let nonce_ladders = contributions.sealed().to_vec();
    let mut deliveries = BTreeMap::new();
    for (recipient_index, recipient) in quorum.iter().enumerate() {
        let nonce = contributions
            .delivery_for(*recipient)
            .ok_or_else(|| format!("dealer {dealer} omitted recipient {recipient}"))?;
        let nonce = nonce.slots();
        deliveries.insert(
            *recipient,
            vec![
                (
                    factor_values[recipient_index],
                    factor_blindings[recipient_index],
                ),
                (cross_values[recipient_index], cross_masks[recipient_index]),
                nonce[0],
                nonce[1],
                nonce[2],
            ],
        );
    }
    Ok(DealerState {
        public: PublicDeal {
            factor: factor_ladder,
            cross: cross_ladder,
            nonce: nonce_ladders,
        },
        deliveries,
    })
}

fn write_public_deal(mailbox: &Path, dealer: PartyId, public: &PublicDeal) -> HarnessResult<()> {
    let text = format!(
        "factor {}\ncross {}\nnonce0 {}\nnonce1 {}\nnonce2 {}\n",
        encode_ladder(&public.factor),
        encode_ladder(&public.cross),
        encode_ladder(&public.nonce[0]),
        encode_ladder(&public.nonce[1]),
        encode_ladder(&public.nonce[2]),
    );
    atomic_write(&public_path(mailbox, dealer), text.as_bytes())?;
    Ok(())
}

fn write_encrypted_deliveries(
    mailbox: &Path,
    dealer: PartyId,
    deliveries: &BTreeMap<PartyId, Vec<(Scalar, Scalar)>>,
    recipient_keys: &BTreeMap<PartyId, WinnerPublicKey>,
) -> HarnessResult<()> {
    for (recipient, delivered) in deliveries {
        let plaintext = delivered
            .iter()
            .map(|(value, mask)| format!("{} {}", encode_scalar(value), encode_scalar(mask)))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let recipient_key = recipient_keys
            .get(recipient)
            .ok_or_else(|| format!("recipient {recipient} supplied no encryption key"))?;
        let encrypted = encrypt_private(dealer, *recipient, recipient_key, plaintext.as_bytes())?;
        atomic_write(
            &private_path(mailbox, dealer, *recipient),
            encrypted.as_bytes(),
        )?;
    }
    Ok(())
}

fn wait_for_dealers(mailbox: &Path, quorum: &[PartyId]) -> HarnessResult<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let ready = quorum
            .iter()
            .all(|dealer| public_path(mailbox, *dealer).exists())
            && quorum.iter().all(|dealer| {
                quorum
                    .iter()
                    .all(|recipient| private_path(mailbox, *dealer, *recipient).exists())
            });
        if ready {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for sealed dealer mailboxes".into());
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn atomic_write(path: &Path, contents: &[u8]) -> HarnessResult<()> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, contents)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn read_public(path: &Path, threshold: usize) -> HarnessResult<PublicDeal> {
    let text = fs::read_to_string(path)?;
    let rows = text
        .lines()
        .map(|line| {
            line.split_once(' ')
                .ok_or_else(|| format!("{} has a malformed public row", path.display()))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let get = |name: &str| -> HarnessResult<Vec<RistrettoPoint>> {
        let ladder = decode_ladder(
            rows.get(name)
                .ok_or_else(|| format!("{} has no {name} ladder", path.display()))?,
        )?;
        if ladder.len() != threshold + 1 {
            return Err(format!("{} has a malformed {name} ladder", path.display()).into());
        }
        Ok(ladder)
    };
    Ok(PublicDeal {
        factor: get("factor")?,
        cross: get("cross")?,
        nonce: vec![get("nonce0")?, get("nonce1")?, get("nonce2")?],
    })
}

fn read_private(
    path: &Path,
    dealer: PartyId,
    recipient: PartyId,
    recipient_secret: &WinnerPrivateKey,
) -> HarnessResult<Vec<(Scalar, Scalar)>> {
    let encrypted = fs::read_to_string(path)?;
    let plaintext = decrypt_private(dealer, recipient, recipient_secret, &encrypted)?;
    let text = String::from_utf8(plaintext)?;
    let delivered = text
        .lines()
        .map(|line| {
            let (value, mask) = line
                .split_once(' ')
                .ok_or_else(|| format!("{} has a malformed private row", path.display()))?;
            Ok((decode_scalar(value)?, decode_scalar(mask)?))
        })
        .collect::<HarnessResult<Vec<_>>>()?;
    if delivered.len() != PRIVATE_SLOTS {
        return Err(format!("{} has {} private slots", path.display(), delivered.len()).into());
    }
    Ok(delivered)
}

fn public_path(mailbox: &Path, dealer: PartyId) -> PathBuf {
    mailbox.join(format!("dealer-{dealer}.public"))
}

fn private_path(mailbox: &Path, dealer: PartyId, recipient: PartyId) -> PathBuf {
    mailbox.join(format!("dealer-{dealer}-to-{recipient}.sealed"))
}

fn decode_recipient_keys(
    line: &str,
    quorum: &[PartyId],
) -> HarnessResult<BTreeMap<PartyId, WinnerPublicKey>> {
    let encoded = line
        .strip_prefix("KEYS ")
        .ok_or("parent omitted the recipient-key bundle")?;
    let keys = encoded
        .split(',')
        .map(|entry| {
            let (party, point) = entry
                .split_once(':')
                .ok_or("a recipient-key entry is malformed")?;
            Ok((party.parse::<PartyId>()?, decode_public_key(point)?))
        })
        .collect::<HarnessResult<BTreeMap<_, _>>>()?;
    if keys.keys().copied().collect::<Vec<_>>() != quorum {
        return Err("recipient-key bundle does not exactly match the quorum".into());
    }
    Ok(keys)
}

fn private_context(dealer: PartyId, recipient: PartyId) -> Vec<u8> {
    format!("qomm:private-delivery:v1:dealer:{dealer}:recipient:{recipient}").into_bytes()
}

fn private_quote_digest() -> [u8; 32] {
    Sha256::digest(b"qomm:private-delivery:v1").into()
}

// This standalone harness has no operator enrollment service. Every process
// derives the same bounded roster fixture for a dealer; production proof
// parties instead use the persisted, confirmed peer manifest.
fn private_identity(dealer: PartyId) -> SigningKey {
    let seed: [u8; 32] = Sha256::new()
        .chain_update(b"QOMM:HARNESS:PRIVATE-IDENTITY:v1")
        .chain_update(dealer.to_be_bytes())
        .finalize()
        .into();
    SigningKey::from_bytes(&seed)
}

fn private_pq_signer(dealer: PartyId) -> MlDsa65Signer {
    let seed: [u8; 32] = Sha256::new()
        .chain_update(b"QOMM:HARNESS:PRIVATE-PQ:v1")
        .chain_update(dealer.to_be_bytes())
        .finalize()
        .into();
    MlDsa65Signer::from_seed(&seed)
}

fn private_pq_key(dealer: PartyId) -> KeyRecord {
    let signer = private_pq_signer(dealer);
    KeyRecord {
        participant_id: ParticipantId::new(format!("harness-dealer-{dealer}")).unwrap(),
        key_id: KeyId::new(format!("harness-dealer-{dealer}-settlement-v1")).unwrap(),
        suite: AUTH_SUITE,
        key_version: 1,
        purpose: KeyPurpose::SettlementInstruction,
        public_key: signer.public_key(),
        not_before: 1,
        not_after: u64::MAX,
        revoked_at: None,
        rotation_proof: None,
        dekyx_binding: None,
    }
}

fn encrypt_private(
    dealer: PartyId,
    recipient: PartyId,
    recipient_public: &WinnerPublicKey,
    plaintext: &[u8],
) -> HarnessResult<String> {
    let context = private_context(dealer, recipient);
    let signer = private_identity(dealer);
    let pq_signer = private_pq_signer(dealer);
    let envelope = seal_for_winner(
        &recipient.to_string(),
        recipient_public,
        plaintext,
        &context,
        private_quote_digest(),
        &signer,
        &pq_signer,
    )?;
    Ok(format!(
        "v{} {} {} {} {} {} {} {} {} {}\n",
        envelope.version,
        hex::encode(envelope.suite.encode()),
        hex::encode(envelope.kem_ciphertext),
        hex::encode(envelope.nonce),
        hex::encode(envelope.context_digest),
        hex::encode(envelope.quote_digest),
        hex::encode(envelope.ciphertext),
        hex::encode(envelope.taker_public),
        hex::encode(envelope.signature.to_bytes()),
        hex::encode(envelope.pq_signature),
    ))
}

fn decrypt_private(
    dealer: PartyId,
    recipient: PartyId,
    recipient_secret: &WinnerPrivateKey,
    encoded: &str,
) -> HarnessResult<Vec<u8>> {
    let fields = encoded.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 10
        || fields[0] != format!("v{VERSION}")
        || fields[1] != hex::encode(KEM_SUITE.encode())
    {
        return Err("an encrypted private delivery is malformed".into());
    }
    let envelope = WinnerEnvelope {
        version: fields[0][1..].parse()?,
        suite: KEM_SUITE,
        kem_ciphertext: hex::decode(fields[2])?,
        nonce: decode_fixed(fields[3], "nonce")?,
        context_digest: decode_fixed(fields[4], "context digest")?,
        quote_digest: decode_fixed(fields[5], "quote digest")?,
        ciphertext: hex::decode(fields[6])?,
        taker_public: decode_fixed(fields[7], "taker public key")?,
        signature: Signature::from_bytes(&decode_fixed(fields[8], "signature")?),
        pq_signature: hex::decode(fields[9])?,
    };
    let expected = private_identity(dealer).verifying_key();
    let pq_key = private_pq_key(dealer);
    open_if_winner(
        &envelope,
        &recipient.to_string(),
        std::slice::from_ref(recipient_secret),
        &private_context(dealer, recipient),
        private_quote_digest(),
        WinnerSenderAuth {
            ed25519: &expected,
            pq_key: &pq_key,
            valid_at: 1,
        },
    )?
    .ok_or_else(|| "private delivery authentication failed for this recipient".into())
}

fn decode_key(value: &str) -> HarnessResult<Vec<u8>> {
    hex::decode(value).map_err(Into::into)
}

fn decode_public_key(value: &str) -> HarnessResult<WinnerPublicKey> {
    WinnerPublicKey::from_raw(&decode_key(value)?).map_err(Into::into)
}

fn decode_fixed<const N: usize>(value: &str, what: &str) -> HarnessResult<[u8; N]> {
    hex::decode(value)?
        .try_into()
        .map_err(|_| format!("{what} must contain {N} bytes").into())
}

fn same_ladder(left: &[RistrettoPoint], right: &[RistrettoPoint]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.compress() == right.compress())
}

fn tagged() -> Transcript {
    let mut transcript = Transcript::new(b"qomm:distributed-product:v1");
    transcript.append_message(b"context", b"ctx");
    transcript
}

fn encode_point(point: &RistrettoPoint) -> String {
    hex::encode(point.compress().as_bytes())
}

fn decode_point(value: &str) -> HarnessResult<RistrettoPoint> {
    let bytes: [u8; 32] = hex::decode(value)?
        .try_into()
        .map_err(|_| "a compressed point must contain 32 bytes")?;
    CompressedRistretto(bytes)
        .decompress()
        .ok_or_else(|| "invalid compressed ristretto point".into())
}

fn encode_scalar(value: &Scalar) -> String {
    hex::encode(value.to_bytes())
}

fn decode_scalar(value: &str) -> HarnessResult<Scalar> {
    let bytes: [u8; 32] = hex::decode(value)?
        .try_into()
        .map_err(|_| "a scalar must contain 32 bytes")?;
    Option::<Scalar>::from(Scalar::from_canonical_bytes(bytes))
        .ok_or_else(|| "a scalar is not canonical".into())
}

fn encode_ladder(ladder: &[RistrettoPoint]) -> String {
    ladder
        .iter()
        .map(encode_point)
        .collect::<Vec<_>>()
        .join(",")
}

fn decode_ladder(value: &str) -> HarnessResult<Vec<RistrettoPoint>> {
    if value.is_empty() {
        return Err("an empty VSS ladder".into());
    }
    value.split(',').map(decode_point).collect()
}

fn read_line(reader: &mut impl BufRead, what: &str) -> HarnessResult<String> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Err(format!("EOF while reading {what}").into());
    }
    Ok(line.trim().to_string())
}

fn integer_median(values: &[u64]) -> Value {
    let mut values = values.to_vec();
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        json!(values[middle])
    } else {
        json!((values[middle - 1] as f64 + values[middle] as f64) / 2.0)
    }
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        out: repo_root().join("artifacts/distributed_assembly.json"),
        parties: 7,
        threshold: 2,
        repeats: 5,
        group: "ed25519".into(),
    };
    let mut args = std::env::args_os().skip(1);
    while let Some(flag) = args.next() {
        match flag.to_str() {
            Some("--out") => options.out = PathBuf::from(next(&mut args, "--out")?),
            Some("--parties") => {
                options.parties = parse_value(next(&mut args, "--parties")?, "--parties")?
            }
            Some("--threshold") => {
                options.threshold = parse_value(next(&mut args, "--threshold")?, "--threshold")?
            }
            Some("--repeats") => {
                options.repeats = parse_value(next(&mut args, "--repeats")?, "--repeats")?
            }
            Some("--group") => {
                options.group = next(&mut args, "--group")?
                    .into_string()
                    .map_err(|_| "--group is not UTF-8")?
            }
            Some("-h" | "--help") => {
                println!("usage: run_distributed_assembly [--out PATH] [--parties N] [--threshold N] [--repeats N] [--group ed25519]");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {}", flag.to_string_lossy()).into()),
        }
    }
    Ok(options)
}

fn next(args: &mut impl Iterator<Item = OsString>, flag: &str) -> HarnessResult<OsString> {
    args.next()
        .ok_or_else(|| format!("argument {flag} expects one value").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_cannot_read_another_childs_private_material_from_the_filesystem() {
        let first_secret = WinnerPrivateKey::generate().unwrap();
        let second_secret = WinnerPrivateKey::generate().unwrap();
        let second_public = second_secret.public_key().unwrap();
        let plaintext = format!(
            "{} {}\n",
            encode_scalar(&Scalar::from(1_234u64)),
            encode_scalar(&Scalar::from(5_678u64))
        );
        let encrypted = encrypt_private(1, 2, &second_public, plaintext.as_bytes()).unwrap();
        let mailbox = Mailbox::new().unwrap();
        let path = private_path(&mailbox.0, 1, 2);
        atomic_write(&path, encrypted.as_bytes()).unwrap();

        let on_disk = fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains(plaintext.trim()));
        assert!(decrypt_private(1, 2, &first_secret, &on_disk).is_err());
        assert_eq!(
            decrypt_private(1, 2, &second_secret, &on_disk).unwrap(),
            plaintext.as_bytes()
        );
    }

    #[test]
    fn every_dealer_seals_before_any_private_record_is_written() {
        let key = Pedersen::new(b"qomm:test:seal-barrier");
        let quorum = [1, 2, 3];
        let dealer = prepare_dealer(&key, 1, &quorum, 2).unwrap();
        let mailbox = Mailbox::new().unwrap();
        write_public_deal(&mailbox.0, 1, &dealer.public).unwrap();
        assert!(public_path(&mailbox.0, 1).exists());
        assert!(quorum
            .iter()
            .all(|recipient| !private_path(&mailbox.0, 1, *recipient).exists()));
    }
}
