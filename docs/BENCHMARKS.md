# Benchmarks

Compare zero-grep, ripgrep, and optionally instant-grep on the same checkout, query, machine, and output sink. Build in release mode, warm each once, alternate command order, and report the median of at least 9 timed runs. Report machine, OS, corpus commit, file count, corpus bytes, query, and tool versions. Exclude build and index construction time unless separately labeled. Verify output equivalence before comparing time.

`scripts/bench.py` runs an alternating median timing comparison and checks normalized output hashes before timing. Pass `--instant-grep /path/to/instant-grep` to include the open-source reference; the script records its index build time separately. `--reuse-index` is for additional patterns over an already-built index. It does not claim performance wins from a single run.

The checked-in results use Linux kernel commit `ebb7a8dc1e74c705daffe11c2faf38ec2628f838`, release binaries, an Apple M3 Max, warm page cache and warm indexes, and 9 alternating runs per query. Results and the rendered chart are in `benchmarks/`.
