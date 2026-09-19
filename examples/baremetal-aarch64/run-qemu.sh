#!/usr/bin/env bash
# Builds (unless SKIP_BUILD=1) and runs the port's table test under QEMU.
# The serial output is kept in artifacts/qemu.log; the exit status is the one the
# image asked for through semihosting.
set -uo pipefail
cd "$(dirname "$0")"
if [[ "${SKIP_BUILD:-0}" != 1 ]]; then
    bash link.sh || exit 1
fi
log=artifacts/qemu.log
timeout 120 qemu-system-aarch64 -M virt -cpu cortex-a72 -m 1024 -nographic -semihosting -nic none \
    -kernel artifacts/table.elf </dev/null 2>&1 | tee "$log"
status=${PIPESTATUS[0]}
echo "qemu exit status: $status"
if [[ "$status" == 124 ]]; then
    echo "the run did not finish within 120 seconds" >&2
    exit 1
fi
if ! grep -q 'TABLE PASS' "$log"; then
    echo "no TABLE PASS in $log" >&2
    exit 1
fi
exit "$status"
