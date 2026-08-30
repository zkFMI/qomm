use qomm_harness::local_mpc::{maybe_run_party, LocalMpcRun};
use qomm_harness::{parse_value, unique_temp_dir, HarnessResult};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

const PROGRAM: &str = r#"
a = sint.get_input_from(0)
b = sint.get_input_from(1)
x = a.expand_to_vector({n_mults})
y = b.expand_to_vector({n_mults})
arr = sint.Array({n_mults})
arr.assign(x * y)
print_ln('QOMM_RESULT=%s', arr.sum().reveal())
"#;

struct Options {
    root: PathBuf,
    parties: usize,
    threshold: usize,
    mults: usize,
    out: PathBuf,
}

fn main() {
    if maybe_run_party() {
        return;
    }
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    let n = options.parties;
    let t = options.threshold;
    let m = options.mults;
    let capacity = (n as isize - 2 * t as isize - 1) / 2;
    if capacity < 0 {
        return Err("parties and threshold give a negative decoding capacity".into());
    }
    let capacity = capacity as usize;
    let mut arms = Map::new();
    arms.insert(
        "king_honest".into(),
        one_run(&options.root, n, t, m, &[], "kh", false)?,
    );
    arms.insert(
        "honest".into(),
        one_run(&options.root, n, t, m, &[], "h", true)?,
    );
    for count in 1..=capacity + 1 {
        let corrupt = (0..count).collect::<Vec<_>>();
        arms.insert(
            format!("corrupt_{count}"),
            one_run(&options.root, n, t, m, &corrupt, &format!("c{count}"), true)?,
        );
    }
    arms.insert(
        "corrupt_last_party".into(),
        one_run(&options.root, n, t, m, &[n - 1], "cl", true)?,
    );
    arms.insert(
        "too_few_parties".into(),
        one_run(&options.root, 7, t, m, &[], "few", true)?,
    );
    let payload = json!({
        "host": qomm_measure::hosts::this_host(),
        "question": "Naming was rung 4 because the protocol still stopped. Does dropping the king and decoding at every party reach rung 5 --- the answer comes out anyway?",
        "setting": {
            "n_parties": n,
            "threshold": t,
            "multiplications": m,
            "decoding_capacity_on_a_degree_2t_product": capacity,
            "n_over_4t_plus_1": format!("{n} >= {}", 4 * t + 1),
        },
        "arms": arms,
    });
    if let Some(parent) = options
        .out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(&options.out, serde_json::to_string_pretty(&payload)?)?;
    for (name, arm) in payload["arms"].as_object().into_iter().flatten() {
        let flag = if arm["finished"].as_bool().unwrap_or(false)
            && arm["answer_correct"].as_bool().unwrap_or(false)
        {
            "OK "
        } else {
            "STOP"
        };
        println!(
            "{flag} {name:22} answer={} named={} rounds={}",
            qomm_harness::value_display(&arm["answer"]),
            qomm_harness::value_display(&arm["named"]),
            qomm_harness::value_display(&arm["rounds"]),
        );
        for key in ["refused_to_start", "refused_past_capacity"] {
            if !arm[key].is_null() {
                println!("       {key}: {}", qomm_harness::value_display(&arm[key]));
            }
        }
    }
    println!("wrote {}", options.out.display());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn one_run(
    root: &std::path::Path,
    parties: usize,
    threshold: usize,
    mults: usize,
    corrupt: &[usize],
    tag: &str,
    robust: bool,
) -> HarnessResult<Value> {
    let work = unique_temp_dir("qomm-robust-atlas")?;
    let source = work.join("prog.mpc");
    fs::write(&source, PROGRAM.replace("{n_mults}", &mults.to_string()))?;
    let party_files = (0..parties).map(|_| "7\n".to_string()).collect::<Vec<_>>();
    let program = format!("robust{tag}_{}", std::process::id());
    let mut run = LocalMpcRun::new(
        root.canonicalize()?,
        program,
        parties,
        threshold,
        "atlas",
        None,
    )?;
    run.install(&source, &party_files)?;
    let _ = run.compile(128)?;
    let extra = if robust {
        vec!["--options".into(), "robust".into()]
    } else {
        Vec::new()
    };
    let environment = vec![(
        "QOMM_CORRUPT_PLAYER".to_string(),
        corrupt
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(","),
    )];
    let observed = run.execute_stock_observed("atlas-party.x", &extra, &environment)?;
    let _ = fs::remove_dir_all(work);

    let results = tagged_integers(&observed.combined, "QOMM_RESULT=");
    let named = tagged_integers(&observed.combined, "ROBUST_ATLAS_CORRECTED player ");
    let refusal = matching_line(&observed.combined, "robust ATLAS needs n >= 4t+1");
    let past_capacity = matching_line_pair(
        &observed.combined,
        "more than ",
        " parties sent wrong shares",
    );
    let expected = 49usize.saturating_mul(mults);
    let corrupt_i64 = corrupt
        .iter()
        .map(|&value| value as i64)
        .collect::<Vec<_>>();
    let answer = if results.len() == 1 {
        json!(results[0])
    } else {
        json!(results)
    };
    Ok(json!({
        "corrupted": corrupt,
        "finished": observed.ok,
        "answer": answer,
        "expected": expected,
        "answer_correct": results == [expected as i64],
        "named": named,
        "named_exactly_the_corrupted": named == corrupt_i64,
        "refused_to_start": refusal,
        "refused_past_capacity": past_capacity,
        "rounds": observed.party0_rounds,
        "global_mb": observed.global_mb,
    }))
}

fn tagged_integers(text: &str, marker: &str) -> Vec<i64> {
    let mut values = BTreeSet::new();
    for (index, _) in text.match_indices(marker) {
        let value = &text[index + marker.len()..];
        let end = value
            .char_indices()
            .take_while(|(index, ch)| ch.is_ascii_digit() || (*index == 0 && *ch == '-'))
            .last()
            .map_or(0, |(index, ch)| index + ch.len_utf8());
        if let Ok(value) = value[..end].parse() {
            values.insert(value);
        }
    }
    values.into_iter().collect()
}

fn matching_line(text: &str, marker: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let at = line.find(marker)?;
        Some(line[at..].to_string())
    })
}

fn matching_line_pair(text: &str, start: &str, end: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let at = line.find(start)?;
        line[at..].contains(end).then(|| line[at..].to_string())
    })
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        root: PathBuf::new(),
        parties: 9,
        threshold: 2,
        mults: 2_000,
        out: qomm_harness::repo_root().join("artifacts/robust_atlas.json"),
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--root" => options.root = PathBuf::from(value(&raw, &mut index, "--root")?),
            "--parties" => {
                options.parties = parse_value(value(&raw, &mut index, "--parties")?, "--parties")?
            }
            "--threshold" => {
                options.threshold =
                    parse_value(value(&raw, &mut index, "--threshold")?, "--threshold")?
            }
            "--mults" => {
                options.mults = parse_value(value(&raw, &mut index, "--mults")?, "--mults")?
            }
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
            unknown => return Err(format!("unknown argument {unknown}").into()),
        }
        index += 1;
    }
    if options.root.as_os_str().is_empty() {
        return Err("--root is required".into());
    }
    Ok(options)
}

fn value(raw: &[OsString], index: &mut usize, name: &str) -> HarnessResult<OsString> {
    *index += 1;
    raw.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} expects a value").into())
}
