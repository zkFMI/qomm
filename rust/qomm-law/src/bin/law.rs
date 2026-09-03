//! Compile a rule base for one deployment on one date, or say why it cannot be.
//!
//! ```text
//! qomm-law rules/*.law --in JP --for listed-equity --as-of 2026-08-22
//! qomm-law rules/*.law --lint
//! qomm-law rules/*.law --due-before 2027-01-01
//! ```
//!
//! Exit 1 on a refusal, so a build can depend on it.

use std::process::ExitCode;

use qomm_law::check::{compile, due_before, lint};
use qomm_law::{emit, parse, Date};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut files = Vec::new();
    let mut jurisdiction = None;
    let mut instrument = None;
    let mut as_of = None;
    let mut lint_only = false;
    let mut due = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--in" => {
                jurisdiction = args.get(i + 1).cloned();
                i += 2;
            }
            "--for" => {
                instrument = args.get(i + 1).cloned();
                i += 2;
            }
            "--as-of" => {
                as_of = args.get(i + 1).cloned();
                i += 2;
            }
            "--due-before" => {
                due = args.get(i + 1).cloned();
                i += 2;
            }
            "--lint" => {
                lint_only = true;
                i += 1;
            }
            other => {
                files.push(other.to_string());
                i += 1;
            }
        }
    }
    if files.is_empty() {
        eprintln!(
            "usage: qomm-law <rules...> [--lint] [--due-before <date>] \
                   [--in <j> --for <i> --as-of <date>]"
        );
        return ExitCode::from(2);
    }

    // Every file is concatenated so the rule base is one document, and where a
    // line came from is tracked alongside --- a refusal naming a line in a file
    // nobody gave is a refusal nobody can act on.
    let mut source = String::new();
    let mut origins: Vec<(usize, String)> = Vec::new();
    for path in &files {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                origins.push((source.lines().count() + 1, path.clone()));
                source.push_str(&text);
                source.push('\n');
            }
            Err(why) => {
                eprintln!("{path}: {why}");
                return ExitCode::from(2);
            }
        }
    }
    let locate = |line: usize| -> String {
        match origins.iter().rev().find(|(start, _)| *start <= line) {
            Some((start, path)) => format!("{path}:{}", line - start + 1),
            None => format!("line {line}"),
        }
    };
    let base = match parse::parse(&source) {
        Ok(base) => base,
        Err(why) => {
            eprintln!("{}: {}", locate(why.line), why.message);
            return ExitCode::from(1);
        }
    };

    if let Some(when) = due {
        let when = match Date::parse(&when) {
            Ok(d) => d,
            Err(why) => {
                eprintln!("{why}");
                return ExitCode::from(2);
            }
        };
        let rows = due_before(&base, when);
        if rows.is_empty() {
            println!("nothing needs reviewing before {when}");
        }
        for (citation, date) in rows {
            println!("{date}  {citation}");
        }
        return ExitCode::SUCCESS;
    }

    if lint_only || jurisdiction.is_none() {
        let (refusals, notes) = lint(&base);
        for note in &notes {
            println!("note: {}: {note}", locate(note.line()));
        }
        for refusal in &refusals {
            eprintln!("refused: {}: {refusal}", locate(refusal.line()));
        }
        if refusals.is_empty() {
            println!(
                "{} jurisdiction(s), {} article(s), {} deployment(s): \
                      every obligation is answered",
                base.jurisdictions.len(),
                base.articles.len(),
                base.deployments.len()
            );
            return ExitCode::SUCCESS;
        }
        return ExitCode::from(1);
    }

    let as_of = match as_of.as_deref().map(Date::parse) {
        Some(Ok(d)) => d,
        Some(Err(why)) => {
            eprintln!("{why}");
            return ExitCode::from(2);
        }
        None => {
            eprintln!(
                "--as-of <date> is required: a rule base has no \
                             opinion without a date"
            );
            return ExitCode::from(2);
        }
    };
    let (Some(j), Some(k)) = (jurisdiction, instrument) else {
        eprintln!("--in and --for go together");
        return ExitCode::from(2);
    };
    match compile(&base, &j, &k, as_of) {
        Ok(compiled) => {
            print!("{}", emit::markdown(&compiled));
            ExitCode::SUCCESS
        }
        Err(refusals) => {
            for refusal in refusals {
                eprintln!("refused: {}: {refusal}", locate(refusal.line()));
            }
            ExitCode::from(1)
        }
    }
}
