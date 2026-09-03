//! A small language for the rules a deployment has to satisfy.
//!
//! `REGULATION.md` scores five jurisdictions against three requirements and
//! lists, in prose, which evidence discharges which obligation. Prose is the
//! wrong container for it, for three reasons that have nothing to do with
//! elegance.
//!
//! **A citation and a check drift apart silently.** A table saying "PTS
//! authorisation, FIEA art. 30" and a codebase emitting a slot receipt are two
//! artefacts nobody compares. Here an obligation names the evidence that
//! discharges it and the compiler refuses a rule base where one does not ---
//! so the correspondence is one to one because it fails to build otherwise.
//!
//! **A rule that nobody has looked at recently is worse than no rule**, because
//! it is answered with confidence. Every clause carries when it was last
//! reviewed and how often it must be; compiling for a date past that refuses
//! and names the clause. Immediate following of a rule change is not a promise
//! anybody keeps by intending to --- it is a build that stops.
//!
//! **And a verdict without its clause is an opinion.** Every verdict here
//! carries the article it came from, so the output is a citation list rather
//! than an assertion.
//!
//! What this is not: legal advice, and not a claim that the rules encoded are
//! current. It is a place to put them where being out of date is loud.

pub mod check;
pub mod emit;
pub mod parse;

use std::collections::BTreeMap;
use std::fmt;

/// A calendar day, which is all the resolution any of this needs.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Date {
    pub fn parse(text: &str) -> Result<Date, String> {
        let mut parts = text.split('-');
        let mut next = |what: &str| -> Result<i64, String> {
            parts
                .next()
                .ok_or_else(|| format!("a date needs a {what}"))?
                .parse::<i64>()
                .map_err(|_| format!("{text}: {what} is not a number"))
        };
        let (year, month, day) = (next("year")?, next("month")?, next("day")?);
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return Err(format!("{text} is not a date"));
        }
        Ok(Date {
            year: year as i32,
            month: month as u32,
            day: day as u32,
        })
    }

    /// Days since an arbitrary epoch, for comparing and adding. Proleptic
    /// Gregorian, which is exact for every date any statute carries.
    pub fn day_number(&self) -> i64 {
        let (mut y, mut m) = (self.year as i64, self.month as i64);
        if m <= 2 {
            y -= 1;
            m += 12;
        }
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let doy = (153 * (m - 3) + 2) / 5 + self.day as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        // The epoch shift belongs here, and leaving it out made `day_number`
        // 719,468 too large while `from_day_number` added it back --- so dates
        // round-tripped through the wrong millennium and every staleness check
        // silently passed. The tests that caught it are the round trip and one
        // interval computed by hand.
        era * 146_097 + doe - 719_468
    }

    pub fn plus_days(&self, days: i64) -> Date {
        Date::from_day_number(self.day_number() + days)
    }

    pub fn from_day_number(z: i64) -> Date {
        let z = z + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        Date {
            year: (if m <= 2 { y + 1 } else { y }) as i32,
            month: m as u32,
            day: d as u32,
        }
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// One article of one instrument of law, and when anybody last looked at it.
///
/// `reviewed` and `review_every` are the versioning. A rule base is not "kept
/// current" by anybody intending to keep it current; it is kept current by the
/// compiler refusing to answer from a clause nobody has checked inside its own
/// stated interval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Article {
    pub statute: String,
    pub article: String,
    pub in_force: Date,
    pub reviewed: Date,
    pub review_every_days: i64,
    pub line: usize,
}

impl Article {
    pub fn citation(&self) -> String {
        format!("{} art. {}", self.statute, self.article)
    }

    pub fn stale_at(&self, when: Date) -> bool {
        when > self.reviewed.plus_days(self.review_every_days)
    }

    pub fn in_force_at(&self, when: Date) -> bool {
        when >= self.in_force
    }
}

/// What a jurisdiction says about one requirement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Permitted as a matter of rule, not tolerated as a matter of practice.
    Permitted,
    /// Available, with a licence or a perimeter attached.
    Conditional,
    /// Not available on this rail at all.
    Refused,
}

impl Verdict {
    pub fn word(&self) -> &'static str {
        match self {
            Verdict::Permitted => "permitted",
            Verdict::Conditional => "conditional",
            Verdict::Refused => "refused",
        }
    }
}

/// A named thing a deployment needs from a legal system.
#[derive(Clone, Debug)]
pub struct Requirement {
    pub id: String,
    pub says: String,
    pub line: usize,
}

/// A financial instrument and the register that holds title to it.
#[derive(Clone, Debug)]
pub struct Instrument {
    pub id: String,
    pub register: String,
    pub line: usize,
}

/// One verdict, with the clause it came from.
#[derive(Clone, Debug)]
pub struct Finding {
    pub requirement: String,
    pub verdict: Verdict,
    pub article: Article,
    pub note: String,
    pub line: usize,
}

/// Something the operator must do, and what discharges it.
///
/// `discharged_by` empty and `undischarged` empty is the case the compiler
/// refuses: an obligation with neither is one nobody has thought about, and
/// that is exactly the state prose leaves them in.
#[derive(Clone, Debug)]
pub struct Duty {
    pub says: String,
    pub discharged_by: Vec<String>,
    pub undischarged: Option<String>,
    pub line: usize,
}

/// Something the protocol emits, what it is good for, and what it is not.
#[derive(Clone, Debug)]
pub struct Evidence {
    pub id: String,
    pub emitted_by: String,
    pub covers: Vec<String>,
    pub does_not_cover: Vec<String>,
    pub line: usize,
}

/// One jurisdiction's answer for one instrument.
#[derive(Clone, Debug)]
pub struct Deployment {
    pub jurisdiction: String,
    pub instrument: String,
    pub findings: Vec<Finding>,
    pub duties: Vec<Duty>,
    pub line: usize,
}

/// Everything a rule base declares, before it is checked.
#[derive(Clone, Debug, Default)]
pub struct RuleBase {
    pub jurisdictions: BTreeMap<String, String>,
    pub articles: BTreeMap<String, Article>,
    pub requirements: BTreeMap<String, Requirement>,
    pub instruments: BTreeMap<String, Instrument>,
    pub evidence: BTreeMap<String, Evidence>,
    pub deployments: Vec<Deployment>,
}

/// What a deployment must satisfy, on a date, with every verdict cited.
#[derive(Clone, Debug)]
pub struct Compiled {
    pub jurisdiction: String,
    pub jurisdiction_name: String,
    pub instrument: String,
    pub register: String,
    pub as_of: Date,
    pub findings: Vec<CompiledFinding>,
    pub duties: Vec<CompiledDuty>,
}

#[derive(Clone, Debug)]
pub struct CompiledFinding {
    pub requirement: String,
    pub says: String,
    pub verdict: Verdict,
    pub citation: String,
    pub in_force: Date,
    pub reviewed: Date,
    pub note: String,
}

#[derive(Clone, Debug)]
pub struct CompiledDuty {
    pub says: String,
    /// Each entry is `(evidence id, where it is emitted, what it is not good for)`.
    pub discharged_by: Vec<(String, String, Vec<String>)>,
    pub undischarged: Option<String>,
}

impl Compiled {
    /// Whether anything here says the deployment cannot go ahead as written.
    pub fn blocked_by(&self) -> Vec<&CompiledFinding> {
        self.findings
            .iter()
            .filter(|f| f.verdict == Verdict::Refused)
            .collect()
    }

    pub fn undischarged(&self) -> Vec<&CompiledDuty> {
        self.duties
            .iter()
            .filter(|d| d.undischarged.is_some())
            .collect()
    }
}
