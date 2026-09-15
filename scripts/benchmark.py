#!/usr/bin/env python3
"""Measure an identical bounded managed workload; not a universal performance claim."""
import argparse
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile
import time


def run(binary, profile):
    with tempfile.TemporaryDirectory() as directory:
        usage = Path(directory) / "usage.txt"
        start = time.perf_counter_ns()
        result = subprocess.run(
            ["/usr/bin/time", "-f", "%U %S %M", "-o", str(usage),
             str(binary.resolve()), "qualify", "benchmark", profile],
            capture_output=True, text=True, timeout=120, check=True)
        elapsed = (time.perf_counter_ns() - start) / 1e9
        if "QUALIFICATION PASS mode=benchmark" not in result.stdout:
            raise RuntimeError("workload did not complete its assertions")
        user, system, rss = usage.read_text().strip().split()
        return {"wall_seconds": elapsed, "user_seconds": float(user),
                "system_seconds": float(system), "max_rss_kib": int(rss),
                "stdout": result.stdout}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--profile", choices=("workstation", "server"), required=True)
    parser.add_argument("--trials", type=int, default=3)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not 1 <= args.trials <= 50:
        parser.error("trials must be in [1, 50]")
    samples = {"baseline": [], "candidate": []}
    binaries = {"baseline": args.baseline, "candidate": args.candidate}
    for trial in range(args.trials):
        order = ("baseline", "candidate") if trial % 2 == 0 else ("candidate", "baseline")
        for key in order:
            samples[key].append(run(binaries[key], args.profile))
    summary = {key: {"file_bytes": binaries[key].stat().st_size,
                     "median_wall_seconds": statistics.median(s["wall_seconds"] for s in values),
                     "median_max_rss_kib": statistics.median(s["max_rss_kib"] for s in values)}
               for key, values in samples.items()}
    baseline = summary["baseline"]["median_wall_seconds"]
    report = {"schema": 1, "machine": platform.machine(), "platform": platform.platform(),
              "cpu_count": os.cpu_count(), "profile": args.profile, "trials": args.trials,
              "summary": summary, "samples": samples,
              "wall_ratio": summary["candidate"]["median_wall_seconds"] / baseline}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"BENCHMARK RECORDED profile={args.profile} ratio={report['wall_ratio']:.3f} output={args.output}")


if __name__ == "__main__":
    main()
