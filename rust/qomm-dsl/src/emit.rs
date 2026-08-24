//! Emit what a checked rule implies: the circuit, an evaluator, the obligations.
//!
//! They come from one source, so they cannot drift apart. That is the practical
//! reason to have a language here: the audit is a compiler output, and a rule
//! that changes changes its own audit with it.

use std::collections::BTreeMap;

use crate::interval::RuleError;
use crate::parse::{Cmp, Expr};
use crate::rule::Rule;

/// One MP-SPDZ expression per output, over the declared columns.
///
/// MP-SPDZ works on secret vectors, so every declared name is one column and the
/// whole maker set is priced in a single pass --- which is why widening the maker
/// count costs bandwidth and not rounds.
pub fn to_mpc(rule: &Rule) -> BTreeMap<String, String> {
    rule.outputs
        .iter()
        .map(|(name, tree)| (name.clone(), mpc(tree)))
        .collect()
}

fn mpc(node: &Expr) -> String {
    match node {
        Expr::Const(v) => format!("sint({v})"),
        Expr::Name(n) => format!("col_{n}"),
        Expr::Neg(e) => format!("(-{})", mpc(e)),
        Expr::Add(a, b) => format!("({} + {})", mpc(a), mpc(b)),
        Expr::Sub(a, b) => format!("({} - {})", mpc(a), mpc(b)),
        Expr::Mul(a, b) => format!("({} * {})", mpc(a), mpc(b)),
        Expr::Compare(a, op, b) => {
            let (l, r) = (mpc(a), mpc(b));
            match op {
                Cmp::Lt => format!("({l}).__lt__({r})"),
                Cmp::Le => format!("({l}).__le__({r})"),
                Cmp::Gt => format!("({l}).__gt__({r})"),
                Cmp::Ge => format!("({l}).__ge__({r})"),
                Cmp::Eq => format!("({l}).__eq__({r})"),
                Cmp::Ne => format!("(1 - ({l}).__eq__({r}))"),
            }
        }
        // A conjunction is a product of bits, which is one multiplication each
        // and so one round layer --- the reason 'or' is not in the language.
        Expr::And(parts) => {
            let joined: Vec<String> = parts.iter().map(mpc).collect();
            format!("({})", joined.join(" * "))
        }
        Expr::Call(name, args) => {
            let rendered: Vec<String> = args.iter().map(mpc).collect();
            match (name.as_str(), rendered.as_slice()) {
                ("min", [a, b]) => format!("(({a}).__lt__({b}).if_else({a}, {b}))"),
                ("max", [a, b]) => format!("(({a}).__lt__({b}).if_else({b}, {a}))"),
                ("clamp", [v, lo, hi]) => format!(
                    "((({v}).__lt__({lo})).if_else({lo}, (({hi}).__lt__({v})).if_else({hi}, {v})))"
                ),
                ("signed", [side, magnitude]) => {
                    format!("(({side}).if_else({magnitude}, -({magnitude})))")
                }
                _ => format!("/* unreachable: {name} */"),
            }
        }
    }
}

/// Evaluate the rule in the clear. Every circuit run is checked against this,
/// which is what makes a disagreement a bug report rather than a mystery.
pub fn evaluate(
    rule: &Rule,
    bindings: &BTreeMap<String, i128>,
) -> Result<BTreeMap<String, i128>, RuleError> {
    rule.outputs
        .iter()
        .map(|(name, tree)| Ok((name.clone(), eval(tree, bindings)?)))
        .collect()
}

fn eval(node: &Expr, b: &BTreeMap<String, i128>) -> Result<i128, RuleError> {
    Ok(match node {
        Expr::Const(v) => *v,
        Expr::Name(n) => *b
            .get(n)
            .ok_or_else(|| RuleError(format!("'{n}' has no value")))?,
        Expr::Neg(e) => -eval(e, b)?,
        Expr::Add(x, y) => eval(x, b)? + eval(y, b)?,
        Expr::Sub(x, y) => eval(x, b)? - eval(y, b)?,
        Expr::Mul(x, y) => eval(x, b)? * eval(y, b)?,
        Expr::Compare(x, op, y) => {
            let (l, r) = (eval(x, b)?, eval(y, b)?);
            i128::from(match op {
                Cmp::Lt => l < r,
                Cmp::Le => l <= r,
                Cmp::Gt => l > r,
                Cmp::Ge => l >= r,
                Cmp::Eq => l == r,
                Cmp::Ne => l != r,
            })
        }
        Expr::And(parts) => {
            let mut all = 1;
            for part in parts {
                all &= eval(part, b)?;
            }
            all
        }
        Expr::Call(name, args) => {
            let v: Vec<i128> = args.iter().map(|a| eval(a, b)).collect::<Result<_, _>>()?;
            match (name.as_str(), v.as_slice()) {
                ("min", [a, c]) => *a.min(c),
                ("max", [a, c]) => *a.max(c),
                ("clamp", [value, lo, hi]) => (*value).clamp(*lo, *hi),
                ("signed", [side, magnitude]) => {
                    if *side == 1 {
                        *magnitude
                    } else {
                        -*magnitude
                    }
                }
                _ => return Err(RuleError(format!("cannot evaluate {name}"))),
            }
        }
    })
}
