#!/usr/bin/env bash
# Exercise receipt contracts in fake-gate fixtures without recursive host gates.
set -euo pipefail

bash scripts/tests/quality-gate-receipt-tests.sh
bash scripts/tests/quality-gate-receipt-hostile-tests.sh
bash scripts/tests/pre-push-quality-gates-tests.sh
bash scripts/tests/dev2merge-quality-gate-receipt-tests.sh
