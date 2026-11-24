#!/usr/bin/env python3
"""Measure a shapr build over the history of local git repositories.

Runs `shapr -J SHA^..SHA` per non-merge commit, caches the NDJSON per
(repo, commit, binary hash), and reports on it:

  corpus.py run   BIN [REPO...]          fill the cache
  corpus.py stats BIN [REPO...]          kind shares, line-weighted structure, review discount
  corpus.py diff  BIN_A BIN_B [REPO...]  what changed between two builds (exit 1 if anything did)
  corpus.py sample BIN [REPO...]         seeded, repo-stratified logic hunks with their diff text

Repositories come from the arguments, else SHAPR_CORPUS (colon-separated).
The repositories are only read (git log, git show). Stdlib only.
"""

import argparse
import collections
import hashlib
import json
import os
import random
import subprocess
import sys
import threading
from concurrent.futures import ThreadPoolExecutor

# Kinds whose hunks are structure (docs/schema.md, "Hunk kinds").
STRUCTURAL = {"format_only", "comment_only", "rename", "move", "extract", "inline"}
GIT_ENV = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}


def default_cache():
    base = os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache")
    return os.environ.get("SHAPR_CORPUS_CACHE") or os.path.join(base, "shapr", "corpus")


def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True, env=GIT_ENV, check=True).stdout


def file_hash(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()[:12]


def commits(repo, limit):
    """The newest `limit` non-merge, non-root commits, newest first."""
    return git(repo, "log", "--no-merges", "--min-parents=1", f"--max-count={limit}", "--format=%H").split()


def repo_key(repo):
    """A cache directory name for a repo: readable, and unique per path."""
    path = os.path.realpath(repo)
    return f"{os.path.basename(path)}-{hashlib.sha256(path.encode()).hexdigest()[:8]}"


def run_one(binary, repo, sha, path, timeout, attempts=2):
    """Runs shapr on one commit and caches what it printed. A run killed by a
    signal (SIGKILL from memory pressure, ^C) says nothing about the commit:
    it is retried, and never cached, so a later run redoes it."""
    for _ in range(attempts):
        try:
            p = subprocess.run([binary, "-J", "-C", repo, f"{sha}^..{sha}"], capture_output=True, env=GIT_ENV, timeout=timeout)
        except subprocess.TimeoutExpired:
            out = json.dumps({"record": "error", "error": f"timeout after {timeout}s", "code": "timeout"}).encode() + b"\n"
            break
        out = p.stdout
        if terminated(out):
            break
        if p.returncode < 0:
            continue
        # A crash: record it, so the commit counts as an error rather than rerunning forever.
        out += json.dumps({"record": "error", "error": p.stderr.decode(errors="replace")[-500:], "code": p.returncode}).encode() + b"\n"
        break
    else:
        return
    # Unique per writer: two runs filling the same cache must not share a temp file.
    tmp = f"{path}.{os.getpid()}.{threading.get_ident()}.tmp"
    with open(tmp, "wb") as f:
        f.write(out)
    os.replace(tmp, path)


def terminated(data):
    """A whole run ends in its summary or an error record."""
    try:
        return json.loads(data.rstrip().rsplit(b"\n", 1)[-1]).get("record") in ("summary", "error")
    except (ValueError, AttributeError):
        return False


def complete(path):
    """A cached run that was cut short is redone."""
    try:
        with open(path, "rb") as f:
            return terminated(f.read())
    except OSError:
        return False


def load(binary, repos, args):
    """{repo name: [(sha, [records])]}, running shapr for commits not yet cached."""
    bin_hash = file_hash(binary)
    result = {}
    for repo in repos:
        name = os.path.basename(os.path.realpath(repo))
        cache = os.path.join(args.cache_dir, bin_hash, repo_key(repo))
        os.makedirs(cache, exist_ok=True)
        shas = commits(repo, args.limit)
        todo = [s for s in shas if not complete(os.path.join(cache, f"{s}.ndjson"))]
        if todo:
            print(f"{name}: running {len(todo)} of {len(shas)} commits ({bin_hash})", file=sys.stderr)
            with ThreadPoolExecutor(args.jobs) as ex:
                list(ex.map(lambda s: run_one(binary, repo, s, os.path.join(cache, f"{s}.ndjson"), args.timeout), todo))
        rows = []
        for sha in shas:
            try:
                with open(os.path.join(cache, f"{sha}.ndjson")) as f:
                    rows.append((sha, [json.loads(line) for line in f if line.strip()]))
            except FileNotFoundError:
                # Killed by a signal: an error for this run only.
                rows.append((sha, [{"record": "error", "error": "killed by a signal", "code": "signal"}]))
        result[name] = rows
    return result


# --- reading records: the one place that knows the -J schema --------------------------------


def summary(records):
    return next((r for r in records if r.get("record") == "summary"), None)


def hunks(records):
    return [r for r in records if r.get("record") == "hunk"]


def unsized(h):
    """Left out of size: `generated: true` before categories, a generated or vendored category after."""
    return h.get("generated", False) or h.get("category") in ("generated", "vendored")


def code_lines(h):
    return h["code_added"] + h["code_deleted"]


def runs(path, side, runs_):
    return [(path, side, n) for r in runs_ for n in range(r["start"], r["start"] + r["count"])]


def parts(h):
    """A hunk's parts in one shape across schemas: kind, structure, code, lines, facets, detail.

    `kind` is `move_with_changes` for a move into new surroundings, the kind the
    hunk-level schema gave it. `code` is the part's changed code lines: exact for
    logic, extract and inline (their `review_lines` is every code line); the
    rest of the hunk's code lines are shared among its other parts by changed
    lines. A hunk-level record is one part covering its before/after runs.
    """
    if "parts" not in h:
        lines = runs(h["old_file"], "-", [h["before"]]) + runs(h["file"], "+", [h["after"]])
        return [{"kind": h["kind"], "structure": h["kind"] in STRUCTURAL, "code": code_lines(h),
                 "changed": h["added"] + h["deleted"], "lines": lines, "facets": h["facets"], "detail": h["detail"]}]
    out = []
    for p in h["parts"]:
        kind = "move_with_changes" if p["kind"] == "move" and not p["structure"] else p["kind"]
        lines = runs(h["old_file"], "-", p["lines"]["before"]) + runs(h["file"], "+", p["lines"]["after"])
        exact = p["kind"] in ("logic", "extract", "inline")
        out.append({"kind": kind, "structure": p["structure"], "code": p["review_lines"] if exact else None,
                    "changed": len(lines), "lines": lines, "facets": p["facets"], "detail": p["detail"]})
    rest = code_lines(h) - sum(p["code"] for p in out if p["code"] is not None)
    shared = [p for p in out if p["code"] is None]
    weight = sum(p["changed"] for p in shared)
    for p in shared:
        p["code"] = rest * p["changed"] / weight if weight else 0
    return out


def line_kinds(records):
    """{(file, side, line): kind} over the lines each part explains."""
    return {line: p["kind"] for h in hunks(records) for p in parts(h) for line in p["lines"]}


# --- stats ------------------------------------------------------------------------------------


def pct(a, b):
    return f"{100 * a / b:.1f}%" if b else "n/a"


def quantile(xs, q):
    xs = sorted(xs)
    if not xs:
        return float("nan")
    i = q * (len(xs) - 1)
    lo = int(i)
    hi = min(lo + 1, len(xs) - 1)
    return xs[lo] + (xs[hi] - xs[lo]) * (i - lo)


def repo_stats(rows):
    s = {"commits": 0, "errors": 0, "hunks": 0, "kinds": collections.Counter(), "lines": collections.Counter(),
         "logic_unparsed_lines": 0, "parsed_lines": collections.Counter(),
         "changed_lines": collections.Counter(), "discounts": [], "parsed_discounts": []}
    for _sha, records in rows:
        summ = summary(records)
        if summ is None:
            s["errors"] += 1
            continue
        s["commits"] += 1
        for h in hunks(records):
            s["hunks"] += 1
            for p in parts(h):
                s["kinds"][p["kind"]] += 1
                if unsized(h):
                    continue
                s["lines"][p["kind"]] += p["code"]
                s["changed_lines"][p["kind"]] += p["changed"]
                if h["language"] is not None:
                    s["parsed_lines"][p["kind"]] += p["code"]
                elif p["kind"] == "logic":
                    s["logic_unparsed_lines"] += p["code"]
        code = summ["code_added"] + summ["code_deleted"]
        if code:
            s["discounts"].append(1 - summ["review_lines"] / code)
        parsed = [h for h in hunks(records) if h["language"] is not None and not unsized(h)]
        code = sum(code_lines(h) for h in parsed)
        if code:
            s["parsed_discounts"].append(1 - sum(h["review_lines"] for h in parsed) / code)
    return s


def merge(stats):
    total = {"commits": 0, "errors": 0, "hunks": 0, "kinds": collections.Counter(), "lines": collections.Counter(),
             "logic_unparsed_lines": 0, "parsed_lines": collections.Counter(),
         "changed_lines": collections.Counter(), "discounts": [], "parsed_discounts": []}
    for s in stats:
        for k, v in s.items():
            total[k] += v
    return total


def share(counter, kinds=STRUCTURAL):
    total = sum(counter.values())
    return round(sum(v for k, v in counter.items() if k in kinds) / total, 3) if total else None


def distribution(d):
    if not d:
        return {"commits": 0}
    return {
        "commits": len(d),
        "p25": round(quantile(d, 0.25), 3),
        "median": round(quantile(d, 0.5), 3),
        "p75": round(quantile(d, 0.75), 3),
        "mean": round(sum(d) / len(d), 3),
        "share_ge_10pct": round(sum(x >= 0.10 for x in d) / len(d), 3),
        "share_ge_50pct": round(sum(x >= 0.50 for x in d) / len(d), 3),
        "share_zero": round(sum(x <= 0 for x in d) / len(d), 3),
    }


def report(s):
    lines = sum(s["lines"].values())
    return {
        "commits": s["commits"],
        "errors": s["errors"],
        "hunks": s["hunks"],
        "parts": sum(s["kinds"].values()),
        "structural_part_share": share(s["kinds"]),
        "code_lines": round(lines),
        "structural_line_share": share(s["lines"]),
        "structural_changed_line_share": share(s["changed_lines"]),
        "parsed_code_lines": round(sum(s["parsed_lines"].values())),
        "structural_parsed_line_share": share(s["parsed_lines"]),
        "move_with_changes_line_share": share(s["lines"], {"move_with_changes"}),
        "logic_unparsed_line_share": round(s["logic_unparsed_lines"] / lines, 3) if lines else None,
        "kinds": dict(s["kinds"]),
        "line_kinds": {k: round(v) for k, v in s["lines"].items()},
        "discount": distribution(s["discounts"]),
        "parsed_discount": distribution(s["parsed_discounts"]),
    }


def cmd_stats(args):
    data = load(args.binary, args.repos, args)
    per = {name: repo_stats(rows) for name, rows in data.items()}
    out = {name: report(s) for name, s in per.items()}
    out["all"] = report(merge(per.values()))
    if args.json:
        print(json.dumps(out, indent=2))
        return 0
    print(f"{'repo':10} {'commits':>7} {'parts':>6} {'struct':>6}  {'code ln':>7} {'struct':>6} {'unparsed':>8}  "
          f"{'rb/rs ln':>8} {'struct':>6} {'mwc':>5}   discount (rb/rs): mean  >=10%  >=50%")
    for name, r in out.items():
        d = r["parsed_discount"]
        print(f"{name:10} {r['commits']:7} {r['parts']:6} {pct(r['structural_part_share'] or 0, 1):>6}  {r['code_lines']:7} "
              f"{pct(r['structural_line_share'] or 0, 1):>6} {pct(r['logic_unparsed_line_share'] or 0, 1):>8}  "
              f"{r['parsed_code_lines']:8} {pct(r['structural_parsed_line_share'] or 0, 1):>6} "
              f"{pct(r['move_with_changes_line_share'] or 0, 1):>5}   {d.get('mean', 0):5.3f} "
              f"{pct(d.get('share_ge_10pct', 0), 1):>6} {pct(d.get('share_ge_50pct', 0), 1):>6}")
    print("\nkinds (parts / code lines), all repos:")
    for k, n in sorted(out["all"]["kinds"].items(), key=lambda kv: -kv[1]):
        print(f"  {k:18} {n:6} {out['all']['line_kinds'].get(k, 0):8}")
    return 0


# --- diff -------------------------------------------------------------------------------------


def cmd_diff(args):
    a = load(args.binary, args.repos, args)
    b = load(args.binary_b, args.repos, args)
    kinds = collections.Counter()  # part count delta per kind
    lines = collections.Counter()  # (kind_a, kind_b) over lines both builds cover
    uncovered = collections.Counter()  # lines only one build covers: "a" or "b"
    verdicts = collections.Counter()
    samples = collections.defaultdict(list)
    for name in a:
        for (sha, ra), (_, rb) in zip(a[name], b[name]):
            sa, sb = summary(ra), summary(rb)
            if sa is None or sb is None:
                if (sa is None) != (sb is None):
                    verdicts[("error" if sa is None else sa["verdict"], "error" if sb is None else sb["verdict"])] += 1
                continue
            kinds.subtract(p["kind"] for h in hunks(ra) for p in parts(h))
            kinds.update(p["kind"] for h in hunks(rb) for p in parts(h))
            if sa["verdict"] != sb["verdict"]:
                key = (sa["verdict"], sb["verdict"])
                verdicts[key] += 1
                samples[("verdict",) + key].append(f"{name} {sha[:12]}")
            la, lb = line_kinds(ra), line_kinds(rb)
            changed = set()
            for key in la.keys() & lb.keys():
                if la[key] != lb[key]:
                    lines[(la[key], lb[key])] += 1
                    changed.add((la[key], lb[key]))
            for pair in changed:
                samples[("line",) + pair].append(f"{name} {sha[:12]}")
            uncovered["a"] += len(la.keys() - lb.keys())
            uncovered["b"] += len(lb.keys() - la.keys())
    if args.json:
        print(json.dumps({
            "kinds": {k: v for k, v in kinds.items() if v},
            "verdicts": [{"a": x, "b": y, "commits": n} for (x, y), n in verdicts.items()],
            "lines": [{"a": x, "b": y, "lines": n} for (x, y), n in lines.items()],
            "uncovered": dict(uncovered),
            "samples": {" ".join(k): v[: args.samples] for k, v in samples.items()},
        }, indent=2))
    else:
        print("part count delta per kind (B - A):")
        for k, v in sorted(kinds.items(), key=lambda kv: kv[1]):
            if v:
                print(f"  {k:18} {v:+d}")
        print("verdict changes (commits):")
        for (x, y), n in sorted(verdicts.items(), key=lambda kv: -kv[1]):
            print(f"  {x:>9} -> {y:9} {n:5}  e.g. {', '.join(samples[('verdict', x, y)][: args.samples])}")
        print("relabeled lines (covered by both):")
        for (x, y), n in sorted(lines.items(), key=lambda kv: -kv[1]):
            print(f"  {x:>17} -> {y:17} {n:6}  e.g. {', '.join(samples[('line', x, y)][: args.samples])}")
        print(f"lines covered only by A: {uncovered['a']}, only by B: {uncovered['b']}")
    return 1 if (any(kinds.values()) or verdicts or lines) else 0


# --- sample -----------------------------------------------------------------------------------


def hunk_text(repo, sha, h, context=3, max_lines=60):
    """The hunk's lines from `git show`, narrowed to its runs plus context."""
    diff = git(repo, "show", "--no-color", "--format=", f"-U{context}", "-M", sha, "--", h["file"], *([h["old_file"]] if h["old_file"] else []))
    out, old, new, keep = [], 0, 0, False
    lo_b, hi_b = h["before"]["start"] - context, h["before"]["start"] + h["before"]["count"] + context
    lo_a, hi_a = h["after"]["start"] - context, h["after"]["start"] + h["after"]["count"] + context
    for line in diff.splitlines():
        if line.startswith("@@"):
            parts = line.split()
            old = int(parts[1][1:].split(",")[0])
            new = int(parts[2][1:].split(",")[0])
            continue
        if not line or line[0] not in " +-" or line.startswith(("+++", "---")):
            continue
        if line[0] == "-":
            keep = lo_b <= old < hi_b
            old += 1
        elif line[0] == "+":
            keep = lo_a <= new < hi_a
            new += 1
        else:
            keep = lo_b <= old < hi_b or lo_a <= new < hi_a
            old += 1
            new += 1
        if keep:
            out.append(line)
    if len(out) > max_lines:
        out = out[:max_lines] + [f"... ({len(out) - max_lines} more lines)"]
    return "\n".join(out)


def cmd_sample(args):
    data = load(args.binary, args.repos, args)
    rng = random.Random(args.seed)
    paths = {os.path.basename(os.path.realpath(r)): r for r in args.repos}
    per_repo = args.n // len(data)
    for name, rows in data.items():
        pool = [(sha, h, p) for sha, records in rows for h in hunks(records) for p in parts(h)
                if p["kind"] == "logic" and p["code"] and not unsized(h) and h["language"] is not None]
        for sha, h, p in rng.sample(pool, min(per_repo, len(pool))):
            print(json.dumps({"repo": name, "sha": sha, "file": h["file"], "before": h["before"], "after": h["after"],
                              "code_lines": p["code"], "facets": p["facets"], "near": p["detail"].get("near"),
                              "text": hunk_text(paths[name], sha, h)}))
    return 0


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--limit", type=int, default=1000, help="newest non-merge commits per repo (default 1000)")
    p.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2))
    p.add_argument("--timeout", type=int, default=120, help="seconds per commit")
    p.add_argument("--cache-dir", default=default_cache(), help="default: SHAPR_CORPUS_CACHE, else $XDG_CACHE_HOME/shapr/corpus")
    p.add_argument("--json", action="store_true")
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("run", "stats", "diff", "sample"):
        s = sub.add_parser(name)
        s.add_argument("binary")
        if name == "diff":
            s.add_argument("binary_b")
            s.add_argument("--samples", type=int, default=3, help="sample commits per row")
        if name == "sample":
            s.add_argument("--n", type=int, default=200)
            s.add_argument("--seed", type=int, default=1)
        s.add_argument("repos", nargs="*")
    args = p.parse_args()
    args.repos = args.repos or [r for r in os.environ.get("SHAPR_CORPUS", "").split(":") if r]
    if not args.repos:
        p.error("no repositories: pass them as arguments or set SHAPR_CORPUS")
    for attr in ("binary", "binary_b"):
        if hasattr(args, attr):
            setattr(args, attr, os.path.realpath(getattr(args, attr)))
    if args.cmd == "run":
        load(args.binary, args.repos, args)
        return 0
    return {"stats": cmd_stats, "diff": cmd_diff, "sample": cmd_sample}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
