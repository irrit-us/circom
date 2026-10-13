"""Compare this fork's source-to-constraint mapping with Circomscribe's live API.

Run after building: python scripts/verify_circomscribe_api.py path/to/circom
Only small, self-contained Circom 2 circuits are used because the website does
not support includes and runs a different, older compiler revision.
"""
import collections
import ast
import hashlib
import json
import pathlib
import re
import subprocess
import sys
import tempfile
import urllib.request

API = "https://circom-visu-backend.seceng.workers.dev/circom"
CASES = {
    "multiply": """pragma circom 2.0.0;
template M() {
 signal input a;
 signal input b;
 signal output c;
 c <== a*b;
}
component main = M();
""",
    "hint": """pragma circom 2.0.0;
template T() {
 signal input a;
 signal output b;
 b <-- a;
 b === a;
}
component main = T();
""",
    "loop": """pragma circom 2.0.0;
template T(n) {
 signal input a[n];
 signal output b[n];
 for (var i=0; i<n; i++) {
  b[i] <== a[i]*a[i];
 }
}
component main = T(2);
""",
    "num2bits": """pragma circom 2.0.0;
template Num2Bits(n) {
 signal input in;
 signal output out[n];
 var sum = 0;
 var power = 1;
 for (var i=0; i<n; i++) {
  out[i] <-- (in >> i) & 1;
  out[i] * (out[i] - 1) === 0;
  sum += out[i] * power;
  power += power;
 }
 sum === in;
}
component main = Num2Bits(2);
""",
}


SIGNAL = re.compile(r"(?:[A-Za-z_]\w*(?:\([^)]*\))?\.)+([A-Za-z_]\w*(?:\[\d+\])*)")
BARE_SIGNAL = re.compile(r"[A-Za-z_]\w*(?:\[\d+\])*")
PRIME = 21888242871839275222246405745257275088548364400416034343698204186575808495617


def signals(equation):
    """Compare named signal occurrences despite different template-name styles."""
    normalized = SIGNAL.sub(lambda match: match.group(1), equation)
    return collections.Counter(BARE_SIGNAL.findall(normalized))


def equation_signature(equation):
    """Fingerprint a constraint by evaluating its two sides in the BN128 field.

    Multiplying both sides by a nonzero scalar leaves the signature unchanged.
    This is a check of formatted equation content, not a formal equivalence proof.
    """
    normalized = SIGNAL.sub(lambda match: match.group(1), equation)
    names = sorted(set(BARE_SIGNAL.findall(normalized)))
    symbols = {name: f"v{index}" for index, name in enumerate(names)}
    normalized = BARE_SIGNAL.sub(lambda match: symbols[match.group()], normalized)
    left, right = normalized.split("=", 1)
    left = ast.parse(left.strip(), mode="eval").body
    right = ast.parse(right.strip(), mode="eval").body

    def evaluate(node, values):
        if isinstance(node, ast.Constant) and isinstance(node.value, int):
            return node.value
        if isinstance(node, ast.Name):
            return values[node.id]
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.USub):
            return -evaluate(node.operand, values)
        if isinstance(node, ast.BinOp):
            a, b = evaluate(node.left, values), evaluate(node.right, values)
            if isinstance(node.op, ast.Add):
                return a + b
            if isinstance(node.op, ast.Sub):
                return a - b
            if isinstance(node.op, ast.Mult):
                return a * b
        raise ValueError(f"Unexpected equation expression: {ast.dump(node)}")

    residues = []
    for seed in range(1, 6):
        values = {
            symbols[name]: int.from_bytes(
                hashlib.sha256(f"{seed}:{name}".encode()).digest(), "big"
            ) % PRIME for name in names
        }
        residues.append((evaluate(left, values) - evaluate(right, values)) % PRIME)
    first = next((value for value in residues if value), None)
    if first is not None:
        inverse = pow(first, -1, PRIME)
        residues = [(value * inverse) % PRIME for value in residues]
    return tuple(residues)


def remote_constraints(source):
    request = urllib.request.Request(
        API, source.encode(), {
            "Content-Type": "text/plain",
            "Origin": "https://www.circomscribe.dev",
            "Referer": "https://www.circomscribe.dev/",
            "User-Agent": "Mozilla/5.0",
        }, method="POST"
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        data = json.load(response)
    if "constraints" not in data or not isinstance(data["constraints"], list):
        raise ValueError("Circomscribe returned no constraint list")
    constraints = collections.defaultdict(list)
    for entry in data["constraints"]:
        line = entry["line_num"]
        assert entry["line"].strip().rstrip(";") in source.splitlines()[line - 1]
        equation = entry["constraint"]
        constraints[line].append((tuple(sorted(signals(equation).items())), equation_signature(equation)))
    return {line: collections.Counter(items) for line, items in constraints.items()}


def local_constraints(binary, source, directory):
    path = directory / "main.circom"
    path.write_text(source, encoding="utf-8")
    result = subprocess.run(
        [str(binary), str(path), "--line-constraints-json", "-o", str(directory)],
        capture_output=True, text=True, timeout=60,
    )
    if result.returncode:
        raise RuntimeError(result.stderr or result.stdout)
    report = json.loads((directory / "main_line_constraints.json").read_text(encoding="utf-8"))
    return {
        row["line"]: collections.Counter(
            (tuple(sorted(signals(item["equation"]).items())), equation_signature(item["equation"]))
            for item in row["constraints"]
        )
        for row in report["lines"] if row["constraints"]
    }


def main():
    binary = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else pathlib.Path(
        "target/x86_64-pc-windows-gnu/debug/circom.exe"
    ).resolve()
    if not binary.is_file():
        raise SystemExit(f"circom binary not found: {binary}")
    with tempfile.TemporaryDirectory(prefix="circom-api-check-") as temporary:
        base = pathlib.Path(temporary)
        for name, source in CASES.items():
            directory = base / name
            directory.mkdir()
            local = local_constraints(binary, source, directory)
            remote = remote_constraints(source)
            if local != remote:
                raise AssertionError(f"{name}: local={local}, API={remote}")
            print(f"{name}: matched {sum(sum(items.values()) for items in local.values())} constraints across {len(local)} source lines")


if __name__ == "__main__":
    main()
