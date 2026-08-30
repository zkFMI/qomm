//! Native measurement summaries and report rendering.

use serde::Serialize;
use serde_json::{json, Value};

pub fn summarise(samples: &[f64]) -> Value {
    crate::timing_summary(samples)
}

pub fn exact<T: Serialize>(value: T) -> Value {
    json!({"exact": value})
}

pub fn render(summary: &Value, places: usize, unit: &str) -> String {
    if summary.is_number() {
        return format!("{:.*}{unit}", places, summary.as_f64().unwrap_or(0.0));
    }
    if let Some(value) = summary.get("exact") {
        return format!("{}{unit} (exact)", crate::value_display(value));
    }
    let n = summary["n"].as_u64().unwrap_or(0);
    if n == 0 {
        return "—".into();
    }
    let mean = summary["mean"].as_f64().unwrap_or(0.0);
    match summary["sd"].as_f64() {
        Some(sd) => format!("{mean:.places$} ± {sd:.places$}{unit} (n={n})"),
        None => format!("{mean:.places$}{unit} (n=1)"),
    }
}

pub fn scaled(summary: &Value, factor: f64) -> Value {
    if let Some(value) = summary.get("exact").and_then(Value::as_f64) {
        return json!({"exact": value * factor});
    }
    let mut output = summary.clone();
    for key in ["mean", "sd", "median", "min", "max"] {
        if let Some(value) = output[key].as_f64() {
            output[key] = json!(value * factor);
        }
    }
    output
}

pub fn value(summary: &Value) -> Option<f64> {
    if summary.is_number() {
        return summary.as_f64();
    }
    summary
        .get("exact")
        .and_then(Value::as_f64)
        .or_else(|| summary.get("mean").and_then(Value::as_f64))
}

pub fn spread(summary: &Value) -> (Option<f64>, usize) {
    if !summary.is_object() || summary.get("exact").is_some() {
        return (None, 1);
    }
    (
        summary.get("sd").and_then(Value::as_f64),
        summary.get("n").and_then(Value::as_u64).unwrap_or(1) as usize,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_match_the_locked_contract() {
        assert_eq!(
            summarise(&[]),
            json!({
                "n": 0, "mean": null, "sd": null, "median": null,
                "min": null, "max": null, "rsd": null,
            })
        );
        assert_eq!(
            summarise(&[2.0]),
            json!({
                "n": 1, "mean": 2.0, "sd": null, "median": 2.0,
                "min": 2.0, "max": 2.0, "rsd": null,
            })
        );
        let repeated = summarise(&[1.0, 3.0]);
        assert_eq!(repeated["n"], 2);
        assert_eq!(repeated["mean"], 2.0);
        assert_eq!(repeated["sd"], 2.0f64.sqrt());
        assert_eq!(repeated["median"], 2.0);
        assert_eq!(repeated["min"], 1.0);
        assert_eq!(repeated["max"], 3.0);
        assert_eq!(repeated["rsd"], 2.0f64.sqrt() / 2.0);
        assert_eq!(summarise(&[-1.0, 1.0])["rsd"], Value::Null);

        assert_eq!(exact(7), json!({"exact": 7}));
        assert_eq!(render(&json!(3), 1, " ms"), "3.0 ms");
        assert_eq!(render(&json!(3.5), 1, " ms"), "3.5 ms");
        assert_eq!(render(&exact(7), 2, " B"), "7 B (exact)");
        assert_eq!(render(&summarise(&[]), 2, " ms"), "—");
        assert_eq!(render(&summarise(&[2.0]), 1, " ms"), "2.0 ms (n=1)");
        assert_eq!(render(&repeated, 1, " ms"), "2.0 ± 1.4 ms (n=2)");

        assert_eq!(scaled(&exact(7), 10.0)["exact"], 70.0);
        let scaled_repeated = scaled(&repeated, 10.0);
        assert_eq!(scaled_repeated["n"], 2);
        assert_eq!(scaled_repeated["mean"], 20.0);
        assert_eq!(scaled_repeated["sd"], 10.0 * 2.0f64.sqrt());
        assert_eq!(scaled_repeated["median"], 20.0);
        assert_eq!(scaled_repeated["min"], 10.0);
        assert_eq!(scaled_repeated["max"], 30.0);
        assert_eq!(scaled_repeated["rsd"], repeated["rsd"]);

        assert_eq!(value(&json!(3)), Some(3.0));
        assert_eq!(value(&exact(7)), Some(7.0));
        assert_eq!(value(&repeated), Some(2.0));
        assert_eq!(value(&Value::Null), None);
        assert_eq!(spread(&exact(3)), (None, 1));
        assert_eq!(spread(&json!(3)), (None, 1));
        assert_eq!(spread(&repeated), (Some(2.0f64.sqrt()), 2));
        assert_eq!(spread(&json!({"sd": null})), (None, 1));
    }

    /// Stable public measurement contract recorded before the superseded
    /// implementation was removed. The fixture is now owned and verified by
    /// this Rust module; no external oracle is executed.
    const MEASUREMENT_CONTRACT: &str = r##"{"exact":{"exact":7},"render":["3.0 ms","3.5 ms","7 B (exact)","\u2014","2.0 ms (n=1)","2.0 \u00b1 1.4 ms (n=2)"],"scaled":[{"exact":70.0},{"max":30.0,"mean":20.0,"median":20.0,"min":10.0,"n":2,"rsd":0.7071067811865476,"sd":14.142135623730951}],"spread":[[null,1],[null,1],[1.4142135623730951,2],[null,1]],"summarise":[{"max":null,"mean":null,"median":null,"min":null,"n":0,"rsd":null,"sd":null},{"max":2.0,"mean":2.0,"median":2.0,"min":2.0,"n":1,"rsd":null,"sd":null},{"max":3.0,"mean":2.0,"median":2.0,"min":1.0,"n":2,"rsd":0.7071067811865476,"sd":1.4142135623730951},{"max":1.0,"mean":0.0,"median":0.0,"min":-1.0,"n":2,"rsd":null,"sd":1.4142135623730951}],"value":[3.0,7.0,2.0,null]}"##;

    #[test]
    fn all_public_operations_match_the_stable_contract() {
        let expected: Value = serde_json::from_str(MEASUREMENT_CONTRACT).unwrap();

        let empty = summarise(&[]);
        let single = summarise(&[2.0]);
        let repeated = summarise(&[1.0, 3.0]);
        let zero_mean = summarise(&[-1.0, 1.0]);
        let spread_json = |summary: &Value| {
            let (sd, n) = spread(summary);
            json!([sd, n])
        };
        let rust = json!({
            "summarise": [empty, single, repeated, zero_mean],
            "exact": exact(7),
            "render": [
                render(&json!(3), 1, " ms"),
                render(&json!(3.5), 1, " ms"),
                render(&exact(7), 2, " B"),
                render(&summarise(&[]), 2, " ms"),
                render(&summarise(&[2.0]), 1, " ms"),
                render(&summarise(&[1.0, 3.0]), 1, " ms"),
            ],
            "scaled": [scaled(&exact(7), 10.0),
                       scaled(&summarise(&[1.0, 3.0]), 10.0)],
            "value": [value(&json!(3)), value(&exact(7)),
                      value(&summarise(&[1.0, 3.0])), value(&Value::Null)],
            "spread": [spread_json(&exact(3)), spread_json(&json!(3)),
                       spread_json(&summarise(&[1.0, 3.0])),
                       spread_json(&json!({"sd": null}))],
        });
        assert_eq!(rust, expected);
    }
}
