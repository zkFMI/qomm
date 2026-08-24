//! What the compiler refuses, which is the whole of its value.

use qomm_law::check::{compile, due_before, lint, Note, Refusal};
use qomm_law::{parse, Date, Verdict};

const BASE: &str = r#"
jurisdiction XX "Nowhere"
requirement R1 "something the law has to allow"
instrument thing register a-register

statute ACT "An Act" {
  article "1" in-force 2020-01-01 reviewed 2026-08-01 every 180d
}

evidence a-receipt from "somewhere.rs"
  covers "keep a record"
  not "saying who"

in XX for thing {
  R1 conditional by ACT "1" "a note"
  obligation "keep a record" discharged-by a-receipt
  obligation "name the sender" undischarged "the design cannot"
}
"#;

// --- dates, because everything else rests on them -------------------------

#[test]
fn a_day_number_round_trips() {
    for text in [
        "1970-01-01",
        "2000-02-29",
        "2020-12-31",
        "2026-08-01",
        "2027-01-28",
        "1900-03-01",
    ] {
        let d = Date::parse(text).unwrap();
        assert_eq!(Date::from_day_number(d.day_number()), d, "{text}");
        assert_eq!(d.to_string(), text);
    }
}

#[test]
fn adding_days_lands_where_a_calendar_says() {
    let d = Date::parse("2026-08-01").unwrap();
    assert_eq!(d.plus_days(1).to_string(), "2026-08-02");
    assert_eq!(d.plus_days(31).to_string(), "2026-09-01");
    // 180 days from 1 August 2026 is 28 January 2027
    assert_eq!(d.plus_days(180).to_string(), "2027-01-28");
    // and a leap year is a leap year
    assert_eq!(
        Date::parse("2028-02-28").unwrap().plus_days(1).to_string(),
        "2028-02-29"
    );
}

#[test]
fn dates_order_the_way_dates_do() {
    let a = Date::parse("2026-08-01").unwrap();
    let b = Date::parse("2027-01-28").unwrap();
    assert!(a < b);
    assert!(b.day_number() - a.day_number() == 180);
}

// --- the refusals ---------------------------------------------------------

#[test]
fn a_clause_nobody_has_reviewed_stops_the_build() {
    let base = parse::parse(BASE).unwrap();
    let fine = compile(&base, "XX", "thing", Date::parse("2027-01-28").unwrap());
    assert!(
        fine.is_ok(),
        "the day it falls due is still inside the interval"
    );

    let stale = compile(&base, "XX", "thing", Date::parse("2027-01-29").unwrap()).unwrap_err();
    assert!(matches!(stale[0], Refusal::Stale { .. }), "{:?}", stale);
    let rendered = stale[0].to_string();
    assert!(rendered.contains("ACT art. 1"), "{rendered}");
    assert!(rendered.contains("2027-01-28"), "{rendered}");
}

#[test]
fn a_clause_that_is_not_yet_in_force_is_refused() {
    let base = parse::parse(BASE).unwrap();
    let early = compile(&base, "XX", "thing", Date::parse("2019-01-01").unwrap()).unwrap_err();
    assert!(
        early
            .iter()
            .any(|r| matches!(r, Refusal::NotYetInForce { .. })),
        "{early:?}"
    );
}

#[test]
fn an_obligation_with_neither_evidence_nor_a_note_does_not_build() {
    let source = BASE.replace(
        "  obligation \"name the sender\" undischarged \"the design cannot\"",
        "  obligation \"name the sender\"",
    );
    let why = parse::parse(&source).unwrap_err();
    assert!(why.message.contains("nobody has thought about"), "{why}");
}

#[test]
fn an_obligation_pointing_at_evidence_that_does_not_exist_is_caught() {
    let source = BASE.replace("discharged-by a-receipt", "discharged-by a-renamed-thing");
    let base = parse::parse(&source).unwrap();
    let (refusals, _) = lint(&base);
    assert!(
        refusals
            .iter()
            .any(|r| matches!(r, Refusal::NoSuchEvidence { .. })),
        "{refusals:?}"
    );
}

#[test]
fn a_requirement_the_deployment_never_answers_is_caught() {
    let source = BASE.replace("  R1 conditional by ACT \"1\" \"a note\"\n", "");
    let base = parse::parse(&source).unwrap();
    let (refusals, _) = lint(&base);
    assert!(
        refusals
            .iter()
            .any(|r| matches!(r, Refusal::RequirementUnanswered { .. })),
        "{refusals:?}"
    );
}

#[test]
fn evidence_that_discharges_nothing_is_a_note_and_not_a_refusal() {
    let source = format!("{BASE}\nevidence spare from \"nowhere.rs\"\n  covers \"nothing\"\n");
    let base = parse::parse(&source).unwrap();
    let (refusals, notes) = lint(&base);
    assert!(refusals.is_empty(), "{refusals:?}");
    assert!(matches!(notes[0], Note::EvidenceCoversNothing { .. }));
}

#[test]
fn a_pair_with_no_rule_is_a_gap_and_not_a_permission() {
    let base = parse::parse(BASE).unwrap();
    let why = compile(
        &base,
        "XX",
        "something-else",
        Date::parse("2026-08-22").unwrap(),
    )
    .unwrap_err();
    assert!(
        why[0].to_string().contains("not a permission"),
        "{:?}",
        why[0]
    );
}

#[test]
fn what_needs_reviewing_can_be_asked_before_the_build_stops() {
    let base = parse::parse(BASE).unwrap();
    assert!(due_before(&base, Date::parse("2027-01-01").unwrap()).is_empty());
    let due = due_before(&base, Date::parse("2027-02-01").unwrap());
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].0, "ACT art. 1");
    assert_eq!(due[0].1.to_string(), "2027-01-28");
}

// --- the language itself --------------------------------------------------

#[test]
fn the_order_the_files_were_given_does_not_matter() {
    // A single pass meant `rules/*.law` worked or not depending on how the
    // shell sorted it, which is the worst kind of intermittent.
    let (declarations, block) = BASE.split_at(BASE.find("in XX").unwrap());
    let reversed = format!("{block}\n{declarations}");
    let base = parse::parse(&reversed).expect("declarations may come last");
    assert_eq!(base.deployments.len(), 1);
}

#[test]
fn a_comment_inside_a_quoted_note_is_not_a_comment() {
    let source = BASE.replace("\"a note\"", "\"a note about art. 30 # and more\"");
    let base = parse::parse(&source).unwrap();
    assert!(base.deployments[0].findings[0].note.contains("# and more"));
}

#[test]
fn an_unknown_statement_is_refused_rather_than_ignored() {
    let why = parse::parse("wibble XX \"nowhere\"").unwrap_err();
    assert!(why.message.contains("not a statement here"), "{why}");
}

#[test]
fn a_block_that_is_never_closed_is_named() {
    let source = BASE.trim_end().trim_end_matches('}');
    let why = parse::parse(source).unwrap_err();
    assert!(why.message.contains("never closed"), "{why}");
}

#[test]
fn a_verdict_carries_the_clause_it_came_from() {
    let base = parse::parse(BASE).unwrap();
    let compiled = compile(&base, "XX", "thing", Date::parse("2026-08-22").unwrap()).unwrap();
    assert_eq!(compiled.findings[0].verdict, Verdict::Conditional);
    assert_eq!(compiled.findings[0].citation, "ACT art. 1");
    assert_eq!(compiled.findings[0].in_force.to_string(), "2020-01-01");
}

#[test]
fn an_obligation_nothing_discharges_survives_into_the_output() {
    let base = parse::parse(BASE).unwrap();
    let compiled = compile(&base, "XX", "thing", Date::parse("2026-08-22").unwrap()).unwrap();
    let open = compiled.undischarged();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].says, "name the sender");
}

// --- the shipped rule base ------------------------------------------------

#[test]
fn the_shipped_rules_lint_clean() {
    let mut source = String::new();
    for name in ["requirements", "japan", "elsewhere"] {
        source.push_str(
            &std::fs::read_to_string(format!("{}/rules/{name}.law", env!("CARGO_MANIFEST_DIR")))
                .unwrap(),
        );
        source.push('\n');
    }
    let base = parse::parse(&source).expect("the shipped rules parse");
    let (refusals, _) = lint(&base);
    assert!(refusals.is_empty(), "{refusals:?}");
    assert_eq!(base.jurisdictions.len(), 5);
}

#[test]
fn the_shipped_rules_reproduce_the_finding_of_regulation_section_3() {
    // Nowhere are all three met at once, and that is the result rather than a
    // bug --- so it is asserted rather than left to be read off a table.
    let mut source = String::new();
    for name in ["requirements", "japan", "elsewhere"] {
        source.push_str(
            &std::fs::read_to_string(format!("{}/rules/{name}.law", env!("CARGO_MANIFEST_DIR")))
                .unwrap(),
        );
        source.push('\n');
    }
    let base = parse::parse(&source).unwrap();
    let as_of = Date::parse("2026-08-22").unwrap();
    let mut permitted_somewhere = std::collections::BTreeMap::new();
    for deployment in &base.deployments {
        let compiled = compile(
            &base,
            &deployment.jurisdiction,
            &deployment.instrument,
            as_of,
        )
        .unwrap();
        let all_three = compiled
            .findings
            .iter()
            .all(|f| f.verdict == Verdict::Permitted);
        assert!(
            !all_three,
            "{} for {} meets all three, which section 3 says \
                             nowhere does",
            deployment.jurisdiction, deployment.instrument
        );
        for finding in &compiled.findings {
            if finding.verdict == Verdict::Permitted {
                permitted_somewhere
                    .entry(finding.requirement.clone())
                    .or_insert_with(Vec::new)
                    .push(deployment.jurisdiction.clone());
            }
        }
    }
    // N2 outright in Switzerland, N3 outright in the UK, N1 nowhere
    assert_eq!(
        permitted_somewhere.get("N2").map(|v| v.as_slice()),
        Some(&["CH".to_string()][..])
    );
    assert_eq!(
        permitted_somewhere.get("N3").map(|v| v.as_slice()),
        Some(&["UK".to_string()][..])
    );
    assert!(
        !permitted_somewhere.contains_key("N1"),
        "no jurisdiction permits a non-disclosing venue outright"
    );
}
