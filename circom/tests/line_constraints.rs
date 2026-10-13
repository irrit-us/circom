use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn workspace() -> PathBuf {
    let id = NEXT.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("circom-line-test-{}-{}", std::process::id(), id));
    fs::create_dir_all(&path).unwrap();
    path
}

fn run(source: &Path, output: &Path, flags: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_circom"));
    command.arg(source).args(flags).arg("-o").arg(output);
    command.output().unwrap()
}

fn report(output: &Path, stem: &str) -> Value {
    let bytes = fs::read(output.join(format!("{}_line_constraints.json", stem))).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn constraints_at<'a>(report: &'a Value, line: u64) -> &'a Vec<Value> {
    report["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["line"] == line && row["file"].as_str().unwrap().ends_with("main.circom"))
        .unwrap()["constraints"]
        .as_array()
        .unwrap()
}

#[test]
fn maps_only_constraint_generating_lines_and_writes_text() {
    let dir = workspace();
    let source = dir.join("main.circom");
    fs::write(&source, "pragma circom 2.0.0;\ntemplate M() {\n signal input a;\n signal input b;\n signal output c;\n signal x;\n x <-- a;\n x === a;\n c <== x * b;\n}\ncomponent main = M();\n").unwrap();
    let result = run(&source, &dir, &["--line-constraints", "--line-constraints-json"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let value = report(&dir, "main");
    assert_eq!(value["stage"], "template constraints before expansion and optimization");
    assert!(constraints_at(&value, 7).is_empty());
    assert_eq!(constraints_at(&value, 8).len(), 1);
    assert_eq!(constraints_at(&value, 9).len(), 1);
    assert_eq!(constraints_at(&value, 9)[0]["callstack"][0], "M");
    let text = fs::read_to_string(dir.join("main_line_constraints.txt")).unwrap();
    assert!(text.contains("    7 |  x <-- a;"));
    assert!(text.contains("    9 |  c <== x * b;"));
    assert!(text.contains("[M()]"));
}

#[test]
fn loop_constraints_share_the_source_line_and_are_stable_across_optimization() {
    let dir = workspace();
    let source = dir.join("main.circom");
    fs::write(&source, "pragma circom 2.0.0;\ntemplate T(n) {\n signal input a[n];\n signal output b[n];\n for (var i = 0; i < n; i++) {\n  b[i] <== a[i] * a[i];\n }\n}\ncomponent main = T(3);\n").unwrap();
    let normal = dir.join("normal");
    let o0 = dir.join("o0");
    fs::create_dir(&normal).unwrap();
    fs::create_dir(&o0).unwrap();
    assert!(run(&source, &normal, &["--line-constraints-json"]).status.success());
    assert!(run(&source, &o0, &["--line-constraints-json", "--O0"]).status.success());
    let a = report(&normal, "main");
    let b = report(&o0, "main");
    assert_eq!(a, b);
    let constraints = constraints_at(&a, 6);
    assert_eq!(constraints.len(), 3);
    for i in 0..3 {
        assert!(constraints[i]["equation"].as_str().unwrap().contains(&format!("a[{}]", i)));
    }
}

#[test]
fn includes_and_standard_r1cs_output_work_together() {
    let dir = workspace();
    let source = dir.join("main.circom");
    fs::write(dir.join("part.circom"), "pragma circom 2.0.0;\ntemplate Part() {\n signal input a;\n signal output b;\n b <== a*a;\n}\n").unwrap();
    fs::write(
        &source,
        "pragma circom 2.0.0;\ninclude \"part.circom\";\ncomponent main = Part();\n",
    )
    .unwrap();
    let result = run(&source, &dir, &["--line-constraints-json", "--r1cs"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(dir.join("main.r1cs").is_file());
    let value = report(&dir, "main");
    let part_line = value["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["file"].as_str().unwrap().ends_with("part.circom") && row["line"] == 5)
        .unwrap();
    assert_eq!(part_line["constraints"].as_array().unwrap().len(), 1);
}

#[test]
fn invalid_circuit_does_not_write_a_misleading_report() {
    let dir = workspace();
    let source = dir.join("main.circom");
    fs::write(&source, "pragma circom 2.0.0;\ntemplate M(){ signal input a; signal output b; b <== a*a*a; }\ncomponent main = M();\n").unwrap();
    let result = run(&source, &dir, &["--line-constraints-json"]);
    assert!(!result.status.success());
    assert!(!dir.join("main_line_constraints.json").exists());
}
