#!/usr/bin/env python3
"""Tests for corpus.py's record reading and build diff. Stdlib only, no git, no network."""

import argparse
import contextlib
import io
import json
import os
import sys
import unittest
import unittest.mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import corpus  # noqa: E402


def run(start, count):
    return {"start": start, "count": count}


def old_hunk(kind, before, after, code=2, added=1, deleted=1):
    """A hunk record from before parts: one kind for the whole hunk."""
    return {"record": "hunk", "file": "a.rb", "old_file": "a.rb", "language": "ruby", "generated": False,
            "before": before, "after": after, "kind": kind, "added": added, "deleted": deleted,
            "code_added": code // 2, "code_deleted": code - code // 2, "review_lines": 0 if kind == "format_only" else code,
            "facets": [], "detail": {}}


def part(kind, before, after, review, structure=None):
    return {"kind": kind, "structure": kind != "logic" if structure is None else structure,
            "lines": {"before": before, "after": after}, "review_lines": review, "facets": [], "detail": {}}


def new_hunk(parts_, before, after, code):
    return {"record": "hunk", "file": "a.rb", "old_file": "a.rb", "language": "ruby", "generated": False,
            "before": before, "after": after, "kind": "logic", "verdict": "mixed", "added": 0, "deleted": 0,
            "code_added": code, "code_deleted": 0, "review_lines": sum(p["review_lines"] for p in parts_),
            "parts": parts_}


def summary(verdict):
    return {"record": "summary", "verdict": verdict}


class Parts(unittest.TestCase):
    def test_a_hunk_level_record_is_one_part_over_its_runs(self):
        [p] = corpus.parts(old_hunk("rename", run(3, 2), run(3, 1), code=3))
        self.assertEqual((p["kind"], p["structure"], p["code"], p["changed"]), ("rename", True, 3, 2))
        self.assertEqual(p["lines"], [("a.rb", "-", 3), ("a.rb", "-", 4), ("a.rb", "+", 3)])

    def test_parts_take_their_own_lines_and_a_changed_move_keeps_its_old_name(self):
        h = new_hunk([part("move", [run(1, 2)], [], 1, structure=False), part("logic", [], [run(5, 1)], 1)],
                     run(1, 5), run(1, 5), code=3)
        moved, logic = corpus.parts(h)
        self.assertEqual((moved["kind"], moved["structure"]), ("move_with_changes", False))
        self.assertEqual(logic["lines"], [("a.rb", "+", 5)])
        # logic's code lines are its review_lines; the rest of the hunk's are the move's
        self.assertEqual((logic["code"], moved["code"]), (1, 2))

    def test_a_part_with_assumptions_is_unproven_and_a_rename_reviews_every_line(self):
        ren = part("rename", [run(1, 1)], [run(1, 1)], 2, structure=False)
        ren["assumptions"] = ["public_api"]
        [p] = corpus.parts(new_hunk([ren], run(1, 1), run(1, 1), code=2))
        self.assertEqual((p["kind"], p["structure"], p["code"]), ("rename_unproven", False, 2))

    def test_unreviewed_code_is_shared_by_changed_lines(self):
        h = new_hunk([part("format_only", [run(1, 1)], [run(1, 1)], 0), part("rename", [run(4, 1)], [], 0)],
                     run(1, 4), run(1, 4), code=6)
        fmt, ren = corpus.parts(h)
        self.assertEqual((fmt["code"], ren["code"]), (4, 2))


class Unsized(unittest.TestCase):
    def test_generated_reads_the_bool_or_the_category(self):
        h = new_hunk([], run(1, 1), run(1, 1), code=1)
        self.assertFalse(corpus.unsized(h))
        self.assertTrue(corpus.unsized({**h, "generated": True}))
        del h["generated"]
        self.assertFalse(corpus.unsized({**h, "category": "test"}))
        self.assertTrue(corpus.unsized({**h, "category": "vendored"}))


class LineKinds(unittest.TestCase):
    def test_new_schema_labels_only_the_lines_a_part_explains(self):
        h = new_hunk([part("comment_only", [], [run(2, 1)], 0), part("logic", [run(4, 1)], [], 1)],
                     run(2, 3), run(2, 1), code=1)
        self.assertEqual(corpus.line_kinds([h]), {("a.rb", "+", 2): "comment_only", ("a.rb", "-", 4): "logic"})


class Terminated(unittest.TestCase):
    def test_a_run_counts_only_when_it_ends_in_a_summary_or_error(self):
        self.assertTrue(corpus.terminated(b'{"record":"hunk"}\n{"record":"summary"}\n'))
        self.assertTrue(corpus.terminated(b'{"record":"error","code":2}\n'))
        self.assertFalse(corpus.terminated(b'{"record":"hunk"}\n'))
        self.assertFalse(corpus.terminated(b""))


class RunOne(unittest.TestCase):
    def run_one(self, *results):
        """Runs corpus.run_one against canned subprocess results; returns the cache path and the call count."""
        import subprocess
        import tempfile
        calls = []

        def fake(*_args, **_kwargs):
            result = results[len(calls)]
            calls.append(result)
            if isinstance(result, Exception):
                raise result
            return subprocess.CompletedProcess([], result[0], stdout=result[1], stderr=b"boom")

        tmp = tempfile.mkdtemp()
        path = os.path.join(tmp, "s.ndjson")
        with unittest.mock.patch.object(corpus.subprocess, "run", fake):
            corpus.run_one("shapr", "repo", "s", path, 5)
        return path, len(calls)

    def test_a_signal_kill_is_retried_and_never_cached(self):
        path, calls = self.run_one((-9, b""), (-9, b""))
        self.assertEqual(calls, 2)
        self.assertFalse(os.path.exists(path))
        self.assertFalse(corpus.complete(path))

    def test_a_signal_kill_retried_into_a_summary_is_cached(self):
        path, calls = self.run_one((-9, b""), (0, b'{"record":"summary"}\n'))
        self.assertEqual(calls, 2)
        self.assertTrue(corpus.complete(path))

    def test_a_crash_and_a_timeout_are_cached_as_errors(self):
        import subprocess
        path, _ = self.run_one((101, b""))
        with open(path) as f:
            self.assertEqual(json.loads(f.read())["code"], 101)
        path, _ = self.run_one(subprocess.TimeoutExpired("shapr", 5))
        with open(path) as f:
            self.assertEqual(json.loads(f.read())["code"], "timeout")

    def test_an_uncached_commit_loads_as_an_error_for_this_run(self):
        import tempfile
        cache = tempfile.mkdtemp()
        args = argparse.Namespace(cache_dir=cache, limit=1, jobs=1, timeout=5)
        with unittest.mock.patch.object(corpus, "commits", lambda _repo, _limit: ["s"]), \
                unittest.mock.patch.object(corpus, "file_hash", lambda _path: "h"), \
                unittest.mock.patch.object(corpus, "run_one", lambda *_args: None), \
                contextlib.redirect_stderr(io.StringIO()):
            rows = corpus.load("shapr", ["repo"], args)
        [(sha, [record])] = rows["repo"]
        self.assertEqual((sha, record["record"]), ("s", "error"))


class Diff(unittest.TestCase):
    def diff(self, a, b):
        data = {"A": {"r": a}, "B": {"r": b}}
        args = argparse.Namespace(binary="A", binary_b="B", repos=[], json=True, samples=3)
        out = io.StringIO()
        with unittest.mock.patch.object(corpus, "load", lambda binary, _repos, _args: data[binary]):
            with contextlib.redirect_stdout(out):
                code = corpus.cmd_diff(args)
        return code, json.loads(out.getvalue())

    def test_identical_builds_differ_in_nothing(self):
        rows = [("s1", [old_hunk("logic", run(1, 1), run(1, 1)), summary("behavior")])]
        code, out = self.diff(rows, rows)
        self.assertEqual(code, 0)
        self.assertEqual((out["kinds"], out["verdicts"], out["lines"]), ({}, [], []))

    def test_an_old_schema_build_diffs_against_a_parts_build_line_by_line(self):
        a = [("s1", [old_hunk("logic", run(1, 3), run(1, 3)), summary("behavior")])]
        b = [("s1", [new_hunk([part("comment_only", [run(1, 1)], [run(1, 1)], 0), part("logic", [run(3, 1)], [run(3, 1)], 2)],
                              run(1, 3), run(1, 3), code=2), summary("mixed")])]
        code, out = self.diff(a, b)
        self.assertEqual(code, 1)
        self.assertEqual(out["kinds"], {"comment_only": 1})
        self.assertEqual(out["verdicts"], [{"a": "behavior", "b": "mixed", "commits": 1}])
        self.assertEqual(out["lines"], [{"a": "logic", "b": "comment_only", "lines": 2}])
        # the old record labels its context line 2 on each side; parts label changed lines only
        self.assertEqual(out["uncovered"], {"a": 2, "b": 0})
        self.assertEqual(out["samples"]["verdict behavior mixed"], ["r s1"])


if __name__ == "__main__":
    unittest.main()
