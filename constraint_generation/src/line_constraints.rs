//! Source-level, pre-expansion constraint report. This deliberately reads the
//! executed templates before DAG flattening and constraint simplification.
use crate::execution_data::ExecutedProgram;
use circom_algebra::algebra::Constraint;
use num_bigint::BigInt;
use program_structure::file_definition::FileLibrary;
use program_structure::constants::UsefulConstants;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{self, Write};

struct Entry {
    template: String,
    callstack: Vec<String>,
    equation: String,
    span: [usize; 2],
}

pub fn write_reports(
    program: &ExecutedProgram,
    files: &FileLibrary,
    text_path: Option<&str>,
    json_path: Option<&str>,
) -> io::Result<()> {
    let prime = UsefulConstants::new(&program.prime);
    let mut by_line: BTreeMap<(usize, usize), Vec<Entry>> = BTreeMap::new();
    for template in &program.model {
        for (constraint, origin) in template.constraints.iter().zip(&template.constraint_origins) {
            let line = files.get_line(origin.start, origin.file_id).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid constraint source location")
            })?;
            by_line.entry((origin.file_id, line)).or_default().push(Entry {
                template: template.report_name.clone(),
                callstack: origin.callstack.clone(),
                equation: format_constraint(constraint, prime.get_p()),
                span: [origin.start, origin.end],
            });
        }
    }

    let mut text = String::new();
    let mut json_lines: Vec<Value> = Vec::new();
    for file_id in 0..files.len() {
        let (raw_name, source) = files
            .name_and_source(file_id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid source file id"))?;
        // The upstream parser stores PathBuf's Debug representation as its file name.
        let name = serde_json::from_str::<String>(&raw_name).unwrap_or(raw_name);
        text.push_str(&format!("{}\n", name));
        for (index, source_line) in source.lines().enumerate() {
            let line = index + 1;
            let entries = by_line.remove(&(file_id, line)).unwrap_or_default();
            text.push_str(&format!("{:>5} | {}\n", line, source_line));
            let mut constraints = Vec::new();
            for entry in entries {
                text.push_str(&format!("      |   [{}] {}\n", entry.template, entry.equation));
                constraints.push(json!({
                    "template": entry.template,
                    "callstack": entry.callstack,
                    "equation": entry.equation,
                    "span": entry.span,
                }));
            }
            json_lines.push(json!({
                "file": name,
                "line": line,
                "source": source_line,
                "constraints": constraints,
            }));
        }
    }
    if !by_line.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "constraint outside source lines"));
    }
    if let Some(path) = text_path {
        File::create(path)?.write_all(text.as_bytes())?;
    }
    if let Some(path) = json_path {
        let report = json!({
            "version": 1,
            "stage": "template constraints before expansion and optimization",
            "lines": json_lines,
        });
        serde_json::to_writer_pretty(File::create(path)?, &report)?;
    }
    Ok(())
}

fn format_constraint(constraint: &Constraint<String>, prime: &BigInt) -> String {
    let a = format_linear(constraint.a(), prime);
    let b = format_linear(constraint.b(), prime);
    let c = format_linear(constraint.c(), prime);
    if constraint.a().is_empty() && constraint.b().is_empty() {
        format!("0 = {}", c)
    } else {
        format!("({}) * ({}) = {}", a, b, c)
    }
}

fn format_linear(coefficients: &HashMap<String, BigInt>, prime: &BigInt) -> String {
    let mut symbols: BTreeSet<&String> = BTreeSet::new();
    symbols.extend(coefficients.keys());
    let mut rendered = String::new();
    for symbol in symbols {
        let mut coefficient = coefficients[symbol].clone();
        if &coefficient > &(prime / 2) {
            coefficient -= prime;
        }
        if coefficient == BigInt::from(0) {
            continue;
        }
        let negative = coefficient < BigInt::from(0);
        if negative {
            coefficient = -coefficient;
        }
        let term = if symbol.is_empty() {
            coefficient.to_string()
        } else if coefficient == BigInt::from(1) {
            symbol.to_string()
        } else {
            format!("{}*{}", coefficient, symbol)
        };
        if rendered.is_empty() {
            if negative {
                rendered.push('-');
            }
        } else if negative {
            rendered.push_str(" - ");
        } else {
            rendered.push_str(" + ");
        }
        rendered.push_str(&term);
    }
    if rendered.is_empty() {
        "0".to_string()
    } else {
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sorted_signed_linear_expression() {
        let mut terms = HashMap::new();
        terms.insert("z".to_string(), BigInt::from(2));
        terms.insert("a".to_string(), BigInt::from(96));
        terms.insert("".to_string(), BigInt::from(1));
        assert_eq!(format_linear(&terms, &BigInt::from(97)), "1 - a + 2*z");
    }
}
