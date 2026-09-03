//! What the compiler refuses, which is the whole of its value.
//!
//! Three refusals, and each exists because the failure it catches is one that
//! prose leaves invisible.
//!
//! **An obligation with nothing discharging it and no note saying why.** In a
//! table that reads as a blank cell and nobody notices. Here it does not build.
//!
//! **A clause nobody has looked at inside its own stated interval.** Answering
//! from it would be answering with confidence from a rule that may have
//! changed, which is worse than not answering. Compiling for a date past the
//! review window refuses and names the article --- so following a rule change
//! immediately is a build that stops rather than an intention somebody holds.
//!
//! **Evidence that covers nothing.** Not fatal, because a module can be added
//! before the obligation it will discharge is written down, but it is reported,
//! because the usual cause is that the obligation was renamed on one side.

use std::collections::BTreeSet;

use crate::{Compiled, CompiledDuty, CompiledFinding, Date, RuleBase};

#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// An obligation with neither evidence nor a note saying why there is none.
    UnansweredObligation { line: usize, says: String },
    /// An obligation pointing at evidence that does not exist.
    NoSuchEvidence {
        line: usize,
        says: String,
        evidence: String,
    },
    /// A clause nobody has reviewed inside its own interval.
    Stale {
        line: usize,
        citation: String,
        reviewed: Date,
        due: Date,
        as_of: Date,
    },
    /// A clause that is not in force on the date being compiled for.
    NotYetInForce {
        line: usize,
        citation: String,
        in_force: Date,
        as_of: Date,
    },
    /// A requirement the rule base declares and this deployment never answers.
    RequirementUnanswered { line: usize, requirement: String },
    /// No rule for the pair being asked about.
    NoSuchDeployment {
        jurisdiction: String,
        instrument: String,
    },
}

impl Refusal {
    /// Where in the concatenated rule base this came from. The caller knows
    /// which file that is; a refusal that cannot say where is a refusal nobody
    /// can act on, so the two halves are printed together and neither is
    /// hard-coded into the other.
    pub fn line(&self) -> usize {
        match self {
            Refusal::UnansweredObligation { line, .. }
            | Refusal::NoSuchEvidence { line, .. }
            | Refusal::Stale { line, .. }
            | Refusal::NotYetInForce { line, .. }
            | Refusal::RequirementUnanswered { line, .. } => *line,
            Refusal::NoSuchDeployment { .. } => 0,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::UnansweredObligation { says, .. } => write!(
                f,
                "the obligation \"{says}\" has nothing discharging it \
                 and no note saying why. An obligation with neither is one nobody \
                 has thought about."
            ),
            Refusal::NoSuchEvidence { says, evidence, .. } => write!(
                f,
                "\"{says}\" is discharged by {evidence}, which is not \
                 declared. The usual cause is a rename on one side."
            ),
            Refusal::Stale {
                citation,
                reviewed,
                due,
                as_of,
                ..
            } => write!(
                f,
                "{citation} was last reviewed {reviewed} and was due \
                 again by {due}; it is now {as_of}. Answering from a clause nobody \
                 has checked is worse than not answering."
            ),
            Refusal::NotYetInForce {
                citation,
                in_force,
                as_of,
                ..
            } => write!(
                f,
                "{citation} comes into force {in_force} and the date \
                 asked about is {as_of}."
            ),
            Refusal::RequirementUnanswered { requirement, .. } => write!(
                f,
                "this deployment never says anything about \
                 {requirement}, which the rule base declares."
            ),
            Refusal::NoSuchDeployment {
                jurisdiction,
                instrument,
            } => write!(
                f,
                "there is no rule for {instrument} in {jurisdiction}. That is a \
                 gap in the rule base, not a permission."
            ),
        }
    }
}

/// Anything wrong that is not fatal.
#[derive(Debug, PartialEq, Eq)]
pub enum Note {
    EvidenceCoversNothing { line: usize, evidence: String },
}

impl Note {
    pub fn line(&self) -> usize {
        match self {
            Note::EvidenceCoversNothing { line, .. } => *line,
        }
    }
}

impl std::fmt::Display for Note {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Note::EvidenceCoversNothing { evidence, .. } => write!(
                f,
                "{evidence} is declared and discharges nothing. Not \
                 fatal --- a module can exist before the obligation it will \
                 discharge is written down --- but the usual cause is a rename."
            ),
        }
    }
}

/// Everything checkable about a rule base, before any date is chosen.
pub fn lint(base: &RuleBase) -> (Vec<Refusal>, Vec<Note>) {
    let mut refusals = Vec::new();
    let mut used: BTreeSet<&str> = BTreeSet::new();
    for deployment in &base.deployments {
        for duty in &deployment.duties {
            if duty.discharged_by.is_empty() && duty.undischarged.is_none() {
                refusals.push(Refusal::UnansweredObligation {
                    line: duty.line,
                    says: duty.says.clone(),
                });
            }
            for name in &duty.discharged_by {
                if base.evidence.contains_key(name) {
                    used.insert(name);
                } else {
                    refusals.push(Refusal::NoSuchEvidence {
                        line: duty.line,
                        says: duty.says.clone(),
                        evidence: name.clone(),
                    });
                }
            }
        }
        for requirement in base.requirements.values() {
            if !deployment
                .findings
                .iter()
                .any(|f| f.requirement == requirement.id)
            {
                refusals.push(Refusal::RequirementUnanswered {
                    line: deployment.line,
                    requirement: requirement.id.clone(),
                });
            }
        }
    }
    let notes = base
        .evidence
        .values()
        .filter(|item| !used.contains(item.id.as_str()))
        .map(|item| Note::EvidenceCoversNothing {
            line: item.line,
            evidence: item.id.clone(),
        })
        .collect();
    (refusals, notes)
}

/// What one deployment must satisfy on one date, or why it cannot be said.
pub fn compile(
    base: &RuleBase,
    jurisdiction: &str,
    instrument: &str,
    as_of: Date,
) -> Result<Compiled, Vec<Refusal>> {
    let deployment = base
        .deployments
        .iter()
        .find(|d| d.jurisdiction == jurisdiction && d.instrument == instrument)
        .ok_or_else(|| {
            vec![Refusal::NoSuchDeployment {
                jurisdiction: jurisdiction.to_string(),
                instrument: instrument.to_string(),
            }]
        })?;

    let (mut refusals, _) = lint(base);
    refusals.retain(|r| {
        matches!(
            r,
            Refusal::UnansweredObligation { .. }
                | Refusal::NoSuchEvidence { .. }
                | Refusal::RequirementUnanswered { .. }
        )
    });

    let mut findings = Vec::new();
    for finding in &deployment.findings {
        let article = &finding.article;
        if article.stale_at(as_of) {
            refusals.push(Refusal::Stale {
                line: article.line,
                citation: article.citation(),
                reviewed: article.reviewed,
                due: article.reviewed.plus_days(article.review_every_days),
                as_of,
            });
        }
        if !article.in_force_at(as_of) {
            refusals.push(Refusal::NotYetInForce {
                line: article.line,
                citation: article.citation(),
                in_force: article.in_force,
                as_of,
            });
        }
        findings.push(CompiledFinding {
            requirement: finding.requirement.clone(),
            says: base
                .requirements
                .get(&finding.requirement)
                .map(|r| r.says.clone())
                .unwrap_or_default(),
            verdict: finding.verdict,
            citation: article.citation(),
            in_force: article.in_force,
            reviewed: article.reviewed,
            note: finding.note.clone(),
        });
    }
    if !refusals.is_empty() {
        return Err(refusals);
    }

    let duties = deployment
        .duties
        .iter()
        .map(|duty| CompiledDuty {
            says: duty.says.clone(),
            discharged_by: duty
                .discharged_by
                .iter()
                .filter_map(|name| {
                    base.evidence
                        .get(name)
                        .map(|e| (e.id.clone(), e.emitted_by.clone(), e.does_not_cover.clone()))
                })
                .collect(),
            undischarged: duty.undischarged.clone(),
        })
        .collect();

    Ok(Compiled {
        jurisdiction: jurisdiction.to_string(),
        jurisdiction_name: base
            .jurisdictions
            .get(jurisdiction)
            .cloned()
            .unwrap_or_default(),
        instrument: instrument.to_string(),
        register: base
            .instruments
            .get(instrument)
            .map(|i| i.register.clone())
            .unwrap_or_default(),
        as_of,
        findings,
        duties,
    })
}

/// Every clause that will need looking at again before a given date.
///
/// The point of the review interval is that somebody acts on it before the
/// build stops, so this is the query that lets them.
pub fn due_before(base: &RuleBase, when: Date) -> Vec<(String, Date)> {
    let mut out: Vec<(String, Date)> = base
        .articles
        .values()
        .map(|a| (a.citation(), a.reviewed.plus_days(a.review_every_days)))
        .filter(|(_, due)| *due <= when)
        .collect();
    out.sort_by_key(|(_, due)| *due);
    out
}
