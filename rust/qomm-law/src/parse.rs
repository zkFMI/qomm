//! The surface syntax, which is deliberately small.
//!
//! Line-oriented, one statement a line, braces only where a block genuinely
//! groups. A bigger language would be a bigger thing to get wrong, and what is
//! being written down is citations and verdicts --- there is nothing here that
//! wants an expression grammar.
//!
//! Every construct carries the line it came from, because the compiler's whole
//! job is refusing, and a refusal that cannot say where is a refusal nobody
//! can act on.

use crate::{
    Article, Date, Deployment, Duty, Evidence, Finding, Instrument, Requirement, RuleBase, Verdict,
};

#[derive(Debug, PartialEq, Eq)]
pub struct LawError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for LawError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

fn err<T>(line: usize, message: impl Into<String>) -> Result<T, LawError> {
    Err(LawError {
        line,
        message: message.into(),
    })
}

/// Split a line into words, keeping "quoted phrases" whole.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => {
                if quoted {
                    out.push(std::mem::take(&mut current));
                    quoted = false;
                } else {
                    quoted = true;
                }
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn strip(line: &str) -> &str {
    // `#` starts a comment, and a `#` inside quotes does not. Cheap enough to
    // do properly rather than to regret.
    let mut quoted = false;
    for (index, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return line[..index].trim_end(),
            _ => {}
        }
    }
    line.trim_end()
}

fn duration(text: &str, line: usize) -> Result<i64, LawError> {
    let (number, unit) = text.split_at(text.len().saturating_sub(1));
    let n: i64 = number.parse().map_err(|_| LawError {
        line,
        message: format!("{text} is not a duration"),
    })?;
    match unit {
        "d" => Ok(n),
        "w" => Ok(n * 7),
        "m" => Ok(n * 30),
        "y" => Ok(n * 365),
        _ => err(line, format!("{text}: a duration ends in d, w, m or y")),
    }
}

fn date(text: &str, line: usize) -> Result<Date, LawError> {
    Date::parse(text).map_err(|message| LawError { line, message })
}

/// Two passes, because a rule base split across files must not depend on the
/// order the files were given. Declarations first, then the blocks that refer
/// to them --- a single pass meant `rules/*.law` worked or did not depending on
/// how the shell sorted it, which is the worst kind of intermittent.
type ParsedLine = (usize, Vec<String>);
type DeferredBlock = (Deployment, Vec<ParsedLine>);

pub fn parse(source: &str) -> Result<RuleBase, LawError> {
    let mut base = RuleBase::default();
    let mut deferred: Vec<DeferredBlock> = Vec::new();
    let mut block: Option<DeferredBlock> = None;
    let mut statute: Option<(String, usize)> = None;
    let mut evidence: Option<Evidence> = None;

    for (index, raw) in source.lines().enumerate() {
        let line = index + 1;
        let text = strip(raw);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts = words(trimmed);
        let head = parts[0].as_str();

        // A block ends wherever it started, and nothing nests.
        if head == "}" {
            if let Some(done) = block.take() {
                deferred.push(done);
            } else if statute.take().is_none() {
                return err(line, "a closing brace with nothing open");
            }
            continue;
        }

        // --- inside an evidence block ------------------------------------
        if let Some(item) = evidence.as_mut() {
            match head {
                "covers" => {
                    item.covers.push(rest(&parts, 1, line, "covers what")?);
                    continue;
                }
                "not" => {
                    item.does_not_cover.push(rest(&parts, 1, line, "not what")?);
                    continue;
                }
                _ => {
                    let done = evidence.take().unwrap();
                    base.evidence.insert(done.id.clone(), done);
                }
            }
        }

        // --- inside a statute block --------------------------------------
        if let Some((name, _)) = statute.clone() {
            if head == "article" {
                if parts.len() < 8 {
                    return err(
                        line,
                        "article <n> in-force <date> reviewed <date> \
                                      every <duration>",
                    );
                }
                let article = Article {
                    statute: name.clone(),
                    article: parts[1].clone(),
                    in_force: date(&parts[3], line)?,
                    reviewed: date(&parts[5], line)?,
                    review_every_days: duration(&parts[7], line)?,
                    line,
                };
                let key = format!("{name}:{}", article.article);
                if base.articles.insert(key.clone(), article).is_some() {
                    return err(line, format!("{key} is declared twice"));
                }
                continue;
            }
        }

        // --- inside a deployment block, kept whole for the second pass -----
        if let Some((_, body)) = block.as_mut() {
            body.push((line, parts));
            continue;
        }

        // --- top level ----------------------------------------------------
        match head {
            "jurisdiction" => {
                if parts.len() < 3 {
                    return err(line, "jurisdiction <id> \"name\"");
                }
                base.jurisdictions
                    .insert(parts[1].clone(), parts[2].clone());
            }
            "statute" => {
                if parts.len() < 2 {
                    return err(line, "statute <id> \"name\" {");
                }
                statute = Some((parts[1].clone(), line));
            }
            "requirement" => {
                if parts.len() < 3 {
                    return err(line, "requirement <id> \"what it asks for\"");
                }
                base.requirements.insert(
                    parts[1].clone(),
                    Requirement {
                        id: parts[1].clone(),
                        says: parts[2].clone(),
                        line,
                    },
                );
            }
            "instrument" => {
                if parts.len() < 4 || parts[2] != "register" {
                    return err(line, "instrument <id> register <register>");
                }
                base.instruments.insert(
                    parts[1].clone(),
                    Instrument {
                        id: parts[1].clone(),
                        register: parts[3].clone(),
                        line,
                    },
                );
            }
            "evidence" => {
                if parts.len() < 4 || parts[2] != "from" {
                    return err(line, "evidence <id> from \"where it is emitted\"");
                }
                evidence = Some(Evidence {
                    id: parts[1].clone(),
                    emitted_by: parts[3].clone(),
                    covers: Vec::new(),
                    does_not_cover: Vec::new(),
                    line,
                });
            }
            "in" => {
                if parts.len() < 4 || parts[2] != "for" {
                    return err(line, "in <jurisdiction> for <instrument> {");
                }
                block = Some((
                    Deployment {
                        jurisdiction: parts[1].clone(),
                        instrument: parts[3].clone(),
                        findings: Vec::new(),
                        duties: Vec::new(),
                        line,
                    },
                    Vec::new(),
                ));
            }
            other => return err(line, format!("{other} is not a statement here")),
        }
    }

    if let Some(item) = evidence.take() {
        base.evidence.insert(item.id.clone(), item);
    }
    if let Some((open, _)) = block {
        return err(open.line, "this block is never closed");
    }
    if let Some((name, line)) = statute {
        return err(line, format!("the statute {name} is never closed"));
    }

    // --- second pass: the blocks, now that every name they use exists -----
    for (mut deployment, body) in deferred {
        if !base.jurisdictions.contains_key(&deployment.jurisdiction) {
            return err(
                deployment.line,
                format!("{} is not a declared jurisdiction", deployment.jurisdiction),
            );
        }
        if !base.instruments.contains_key(&deployment.instrument) {
            return err(
                deployment.line,
                format!("{} is not a declared instrument", deployment.instrument),
            );
        }
        for (line, parts) in body {
            let head = parts[0].as_str();
            if head == "obligation" {
                let says = parts.get(1).cloned().ok_or(LawError {
                    line,
                    message: "obligation \"what\"".into(),
                })?;
                let mut duty = Duty {
                    says,
                    discharged_by: Vec::new(),
                    undischarged: None,
                    line,
                };
                match parts.get(2).map(String::as_str) {
                    Some("discharged-by") => {
                        if parts.len() < 4 {
                            return err(
                                line,
                                "discharged-by needs at least one \
                                              evidence name",
                            );
                        }
                        duty.discharged_by = parts[3..].to_vec();
                    }
                    Some("undischarged") => {
                        duty.undischarged = Some(rest(&parts, 3, line, "undischarged \"why\"")?);
                    }
                    _ => {
                        return err(
                            line,
                            "an obligation is either discharged-by \
                                           something or undischarged \"why\" --- \
                                           and one with neither is one nobody has \
                                           thought about",
                        )
                    }
                }
                deployment.duties.push(duty);
                continue;
            }
            if !base.requirements.contains_key(head) {
                return err(
                    line,
                    format!("{head} is not an obligation and not a declared requirement"),
                );
            }
            if parts.len() < 5 || parts[2] != "by" {
                return err(
                    line,
                    format!(
                        "{head} <verdict> by <statute> <article> \
                                          \"note\""
                    ),
                );
            }
            let verdict = match parts[1].as_str() {
                "permitted" => Verdict::Permitted,
                "conditional" => Verdict::Conditional,
                "refused" => Verdict::Refused,
                other => {
                    return err(
                        line,
                        format!("{other} is not a verdict; permitted, conditional or refused"),
                    )
                }
            };
            let key = format!("{}:{}", parts[3], parts[4]);
            let article = base.articles.get(&key).cloned().ok_or(LawError {
                line,
                message: format!("{key} is not a declared article"),
            })?;
            deployment.findings.push(Finding {
                requirement: head.to_string(),
                verdict,
                article,
                note: parts.get(5).cloned().unwrap_or_default(),
                line,
            });
        }
        base.deployments.push(deployment);
    }
    Ok(base)
}

fn rest(parts: &[String], from: usize, line: usize, what: &str) -> Result<String, LawError> {
    parts.get(from).cloned().ok_or(LawError {
        line,
        message: what.into(),
    })
}
