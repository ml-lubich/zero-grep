#!/usr/bin/env python3
"""Render JSON benchmark runs as a GitHub-friendly SVG and Markdown summary."""
import html
import json
import math
from pathlib import Path

root = Path(__file__).resolve().parents[1]
results = sorted((root / "benchmarks/results").glob("linux-*.json"))
if not results:
    raise SystemExit("no linux benchmark result JSON files found")
data = [json.loads(path.read_text()) for path in results]
first = data[0]
cpu = html.escape(first["machine"]["cpu"])
corpus = first["corpus"]
series = [("zero-grep", "#62d6b5"), ("ripgrep", "#8b95a7"), ("instant-grep", "#b68aff")]
width, left, top = 1000, 240, 128
group_h, bar_h, gap = 92, 13, 5
chart_h = top + group_h * len(data) + 24
svg = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {chart_h}" role="img" aria-labelledby="title desc">']
svg += ['<title id="title">Warm search latency on Linux kernel source</title>',
        f'<desc id="desc">Logarithmic benchmark chart on {corpus["files"]:,} files and {corpus["bytes"] / 1e9:.2f} gigabytes, {cpu}. Lower is faster. Nine alternating runs per query.</desc>',
        '<rect width="100%" height="100%" rx="18" fill="#111820"/>',
        '<text x="32" y="42" fill="#f5f7fa" font-family="system-ui,sans-serif" font-size="24" font-weight="700">Search latency · warm index · lower is faster</text>',
        f'<text x="32" y="70" fill="#9ba7b4" font-family="system-ui,sans-serif" font-size="13">Linux kernel {corpus["commit"][:9]} · {corpus["files"]:,} files · {corpus["bytes"] / 1e9:.2f} GB · {cpu} · 9 alternating runs</text>']
plot_w = width - left - 76
for tick in (1, 10, 100, 1000, 10000):
    x = left + math.log10(tick) / 4 * plot_w
    svg.append(f'<line x1="{x:.1f}" y1="{top-20}" x2="{x:.1f}" y2="{chart_h-18}" stroke="#28333e"/>')
    svg.append(f'<text x="{x:.1f}" y="{top-27}" text-anchor="middle" fill="#93a0ae" font-family="system-ui,sans-serif" font-size="11">{tick:,} ms</text>')
for row, item in enumerate(data):
    y = top + row * group_h
    pattern = html.escape(item["pattern"])
    svg.append(f'<text x="32" y="{y+17}" fill="#e8edf2" font-family="ui-monospace,monospace" font-size="14">{pattern}</text>')
    for col, (name, color) in enumerate(series):
        value = item["median_ms"].get(name)
        if value is None:
            continue
        yy = y + 25 + col * (bar_h + gap)
        xw = math.log10(max(value, 1)) / 4 * plot_w
        svg.append(f'<text x="32" y="{yy+11}" fill="{color}" font-family="system-ui,sans-serif" font-size="11">{name}</text>')
        svg.append(f'<rect x="{left}" y="{yy}" width="{max(xw,2):.1f}" height="{bar_h}" rx="6" fill="{color}"/>')
        svg.append(f'<text x="{left+xw+7:.1f}" y="{yy+11}" fill="#e3e8ed" font-family="ui-monospace,monospace" font-size="11">{value:,.1f} ms</text>')
svg.append('</svg>')
(root / "benchmarks/latest.svg").write_text("\n".join(svg) + "\n")

lines = ["# Benchmarks", "", "![Warm search latency chart](latest.svg)", "",
         f"Measured on {cpu} ({first['machine']['os']}), {corpus['files']:,} tracked files / {corpus['bytes']/1e9:.2f} GB, Linux kernel commit `{corpus['commit']}`. Release builds; warm page cache and warm indexes; 9 alternating runs; median wall time. The harness verifies exact output equality with `rg -n` before timing.", "",
         "| Pattern | zero-grep | ripgrep | instant-grep | zero-grep vs rg |", "|---|---:|---:|---:|---:|"]
for item in data:
    times = item["median_ms"]
    z = times["zero-grep"]
    rg = times["ripgrep"]
    instant = times.get("instant-grep")
    instant_text = f"{instant:,.1f} ms" if instant is not None else "not measured"
    lines.append(f"| `{item['pattern']}` | {z:,.1f} ms | {rg:,.1f} ms | {instant_text} | {rg/z:.2f}× |")
build = next((x for x in data if x["index_build_ms"]), None)
if build:
    vals = build["index_build_ms"]
    lines += ["", f"Index build, separately measured on the same corpus: zero-grep {vals['zero-grep']:,.0f} ms; instant-grep {vals['instant-grep']:,.0f} ms."]
lines += ["", "Common terms still scan most files, so the index offers little benefit there. Query regexes with only broad grams also fall back to a full scan. Results are hardware and corpus specific; use the included script to reproduce.", "", "Run: `python3 scripts/bench.py 'ACPI_COMPANION' /path/to/linux --instant-grep /path/to/instant-grep --json /tmp/result.json`."]
(root / "benchmarks/README.md").write_text("\n".join(lines) + "\n")
