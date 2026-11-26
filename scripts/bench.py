#!/usr/bin/env python3
"""Alternating, output-checked wall-time comparison on one pinned corpus."""
import argparse
import hashlib
import json
import os
import platform
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument("pattern")
p.add_argument("path", type=Path)
p.add_argument("--runs", type=int, default=9)
p.add_argument("--instant-grep", type=Path)
p.add_argument("--json", type=Path, help="write measurements to this file")
p.add_argument("--reuse-index", action="store_true", help="use existing indexes; report no build time")
a = p.parse_args()
root = a.path.resolve()
tools = {"zero-grep": ["zg", a.pattern, str(root)], "ripgrep": ["rg", "-n", a.pattern, str(root)]}
if a.instant_grep:
    tools["instant-grep"] = [str(a.instant_grep), "-n", a.pattern, str(root)]

# Collect corpus size from tracked Git files when available; otherwise count visible files.
tracked = subprocess.run(["git", "-C", str(root), "ls-files", "-z"], capture_output=True)
if tracked.returncode == 0:
    paths = [root / os.fsdecode(item) for item in tracked.stdout.split(b"\0") if item]
else:
    paths = [f for f in root.rglob("*") if f.is_file() and ".git" not in f.parts and ".zero-grep" not in f.parts]
corpus = {"files": len(paths), "bytes": sum(f.stat().st_size for f in paths if f.is_file())}

def run(cmd):
    start = time.perf_counter()
    result = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    return (time.perf_counter() - start) * 1000, result

index_build_ms = {}
if not a.reuse_index:
    if "zero-grep" in tools:
        index_build_ms["zero-grep"] = run(["zg", "--index", "-q", a.pattern, str(root)])[0]
    if "instant-grep" in tools:
        index_build_ms["instant-grep"] = run([str(a.instant_grep), "--index", "-q", a.pattern, str(root)])[0]

def output_digest(cmd):
    with tempfile.TemporaryFile() as output:
        proc = subprocess.run(cmd, stdout=output, stderr=subprocess.DEVNULL)
        if proc.returncode not in (0, 1):
            raise SystemExit(f"search failed: {cmd[0]}")
        output.seek(0)
        lines = [line.rstrip(b"\r\n") for line in output if line.strip()]
    digest = hashlib.sha256()
    for line in sorted(lines):
        digest.update(line + b"\n")
    return digest.hexdigest()

digests = {}
for name, cmd in tools.items():
    digests[name] = output_digest(cmd)
if len(set(digests.values())) > 1:
    raise SystemExit("output mismatch; refusing to compare timings")

times = {name: [] for name in tools}
for i in range(a.runs):
    order = list(tools) if i % 2 == 0 else list(reversed(tools))
    for name in order:
        start = time.perf_counter()
        subprocess.run(tools[name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        elapsed = (time.perf_counter() - start) * 1000
        times[name].append(round(elapsed, 3))

cpu_name = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True).stdout.strip() or platform.machine()
memory_raw = subprocess.run(["sysctl", "-n", "hw.memsize"], capture_output=True, text=True).stdout.strip()
memory_gb = round(int(memory_raw) / (1024**3), 1) if memory_raw.isdigit() else None
result = {
    "pattern": a.pattern,
    "corpus": {
        "name": root.name,
        "commit": subprocess.run(["git", "-C", str(root), "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip() or "unversioned",
        **corpus,
    },
    "machine": {
        "os": platform.platform(),
        "cpu": cpu_name,
        "memory_gb": memory_gb,
    },
    "runs": a.runs,
    "index_build_ms": index_build_ms,
    "median_ms": {name: round(statistics.median(values), 3) for name, values in times.items()},
    "samples_ms": times,
    "outputs_match": True,
    "output_sha256": next(iter(digests.values())),
}
print(json.dumps(result, indent=2))
if a.json:
    a.json.write_text(json.dumps(result, indent=2) + "\n")
