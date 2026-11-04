#!/usr/bin/env bash
# Follow cargo-llvm-cov's external-test recipe; isolate instrumented artifacts.
set -euo pipefail
cd "$(dirname "$0")/../.."
out=${1:?usage: coverage.sh /absolute/private/report-directory}
python3 - "$out" <<'PY'
from pathlib import Path
import sys
root=Path.cwd().resolve();out=Path(sys.argv[1]).resolve()
if not Path(sys.argv[1]).is_absolute() or out.is_relative_to(root):raise SystemExit('Keep coverage artifacts outside the source tree')
out.mkdir(parents=True,exist_ok=True)
for name in ['summary.json','coverage.json','coverage.lcov','protocol-cases.json','protocol-cases.xml']:(out/name).unlink(missing_ok=True)
PY
export CARGO_TARGET_DIR="$out/build"
cargo llvm-cov show-env --sh > "$out/coverage-env.sh"
source "$out/coverage-env.sh"
cargo llvm-cov clean --workspace
cargo test --locked --all-targets
python3 tools/testing/protocols.py --binary "$CARGO_TARGET_DIR/debug/rust-irrd" --report "$out/protocol-cases"
cargo llvm-cov report --json --summary-only --output-path "$out/coverage.json"
cargo llvm-cov report --lcov --output-path "$out/coverage.lcov"
cargo llvm-cov report --html --output-dir "$out/html"
python3 tools/testing/coverage_summary.py "$out/coverage.json" --out "$out/summary.json" --minimum 92
