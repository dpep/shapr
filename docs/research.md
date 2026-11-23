# Prior art for shapr

Research survey behind shapr's design and the review tooling planned on top of it. Every linked source was opened and read. Numbers are as the authors reported them, rounded. Evidence strength is marked where it is thin.

## Takeaways

1. **Size is most of the risk signal, and it's cheap.** Lines added on its own, fed to logistic regression, beat two deep-learning JIT models (mean AUC 0.72 vs 0.71). Kamei's 14 git-only features are the right base for the risk model, and "LA only" is the baseline every richer model has to beat. shapr's real contribution is a *better* size count: lines left to review once moves, renames, formatting and comments are discounted.
2. **"Pure structure" doesn't mean safe.** Across 103 Apache projects, refactoring actions were linked to later bug fixes. Manual review then put part of that down to tangled co-changes, but not all of it. Extract Method and Inline Method lead in the newest study. shapr should say *structure*, never *safe*, and the review agent should still look at extracts and inlines.
3. **Mixed changes are a review hazard more than a proven defect hazard.** Reviewers say a refactoring mixed with behavior hides bugs and ask for splits. Refactoring reviews take longer (median roughly 2x). Tangled fixes corrupt history: up to 20% of bug-fix commits are tangled, and at least 16% of files get attributed to the wrong bug. The defect evidence that *mixed* is worse than *pure* is weak and contradictory.
4. **Automatic untangling is mediocre on real commits.** Benchmarks built from synthetic composites flatter tools: matched synthetic composites scored 0.33 ARI higher than real squashed diffs. The best agent setups reach 0.46 ARI on real history. The decomposer should lead with the split shapr can make deterministically (structure vs behavior, then by file/dependency), let an LLM propose intent groups as a draft, and check that every proposed commit replays.
5. **Refactoring detection is mature for Java and immature for dynamic languages.** RefactoringMiner self-reports ≥0.99 precision. Independent Python numbers are much lower: PyRef F1 0.64, ActRef F1 0.85, with extract/inline precision as low as 0.34–0.56. No Ruby detector exists. Borrow techniques rather than tools: GumTree-style top-down/bottom-up matching, and RefDiff's TF-IDF weighted-Jaccard body similarity with per-kind thresholds.
6. **SZZ labels are noisy; shapr can make them cleaner.** About half of the commits SZZ labels as bug fixes aren't. The best variant (R-SZZ) reaches 0.66 precision. Ignoring cosmetic and refactoring hunks is the standard fix, and that is exactly what shapr's output provides. Prefer revert/hotfix labels where they exist.
7. **High-signal AI review comes from suppression and per-category thresholds, not better prompts.** Google's AutoCommenter went from 54% to more than 80% "useful" by dropping comments on unchanged lines, setting a confidence threshold per category, and suppressing non-actionable categories. In an industrial PR-Agent rollout, 74% of comments were resolved, but PR closure time rose from 5.9 h to 8.3 h.
8. **Models drift.** JIT model AUC and calibration drop noticeably about a year after training. Retrain on recent data and log the model version alongside each verdict.

## 1. Features that predict risk

| Finding | Source |
|---|---|
| 14 change features in 5 dimensions: **size** LA, LD, LT; **diffusion** NS, ND, NF, Entropy; **purpose** FIX; **history** NDEV, AGE, NUC; **experience** EXP, REXP, SEXP. All computable from git plus a fix-keyword match. ND and REXP were dropped as collinear; LA/LD were normalized by LT, and LT/NUC by NF. | [Kamei et al. 2013, TSE](https://sailresearch.github.io/sail-website/data/pdfs/TSE_ALarge-ScaleEmpiricalStudyOfJust-in-TimeQualityAssurance.pdf) |
| Logistic regression over 11 projects: 68% accuracy, 64% recall. Effort-aware: reviewing the riskiest 20% of changed LOC catches 35% of defect-inducing changes. Risk-increasing: number of files, FIX, relative churn. Risk-decreasing: time since the last change. Diffusion matters most in commercial code. | same |
| **LApredict** (lines added + logistic regression) beat CC2Vec and DeepJIT on 310k changes: mean within-project AUC 0.72 vs 0.71, with a bigger lead cross-project, and 81,000x faster to train. Deep JIT models don't consistently beat traditional features. | [Zeng et al. 2021, ISSTA](https://zhangyuqun.github.io/publications/issta2021.pdf) |
| CC2Vec's published results trained on the test set. Fixing that cut its F-measure by 38–46%. JITLine (token bag + LR/RF) also gives line-level risk. Treat impressive deep-JIT numbers skeptically. | [Pornprasit & Tantithamthavorn 2021](https://arxiv.org/abs/2103.07068) |
| On 37.5k Qt/OpenStack changes, AUC and Brier score degrade about 1 year after training, and the importance of size and experience features shifts over time. Retrain on recent data; 6 months or more of history steadies the interpretation. | [McIntosh & Kamei 2018, TSE](https://rebels.cs.uwaterloo.ca/journalpaper/2017/04/04/are-fix-inducing-changes-a-moving-target.html) |
| Google, 9M reviewed changes: median change is 24 lines, >35% touch a single file, ~90% touch fewer than 10 files, median of 1 reviewer, median first response under 1 h for small changes and about 5 h for very large ones. Comments per change grow with size up to ~1,250 lines (12.5 comments), then fall off (generated code, big deletions). | [Sadowski et al. 2018, ICSE-SEIP](https://sback.it/publications/icse2018seip.pdf) |

**Cheap (git-only):** all 14 Kamei features, file age, recent churn, author's prior commits to the file, revert/hotfix markers.
**Needs infra we lack:** issue-tracker bug labels (for clean FIX labels), CI and test outcomes, production incident links, coverage. Leave these out of v1.

## 2. Refactoring and change-shape detection

| Tool | Languages | Detects | Accuracy (who measured) | Worth copying |
|---|---|---|---|---|
| [RefactoringMiner](https://github.com/tsantalis/RefactoringMiner) (MIT) | Java, Python, Kotlin, JS, TS, C, C++ | 107 types; also commit/PR AST diff; matches comments that were only reformatted | [Self-reported](https://github.com/tsantalis/RefactoringMiner/blob/master/documentation/accuracy.md): Java P 0.999 / R 0.984 on 12.9k instances; Python 0.997 / 0.998 on 1.2k instances from 3 projects (validation ongoing); JS only 171 instances | Statement matching *after* replacing renamed identifiers; the type catalog |
| [RefDiff](https://arxiv.org/abs/1704.01544) / [RefDiff 2](https://github.com/aserg-ufmg/RefDiff) (MIT) | Java, JS, C, Go (plugins) | 11 relationships: rename, move, move+rename, extract, inline, change signature, pull up/push down, extract supertype, convert type | Java P 1.00 / R 0.88 on 448 instances (paper); P 0.96 / R 0.80 on 3.2k (v2); Go P 0.92 / R 0.80 per RAID | **Code as a token multiset, TF-IDF-weighted Jaccard, thresholds calibrated per relationship**: simple, deterministic, language-agnostic. The best fit for Ruby. |
| [ActRef](https://arxiv.org/abs/2505.06553) | Python | 15 types at method, class, module and variable level | Independent dataset of 1.9k instances: P 0.80 / R 0.92; PyRef on the same data P 0.76 / R 0.55. Extract Method P 0.56, Inline P 0.34. The Python port of RefactoringMiner detected *nothing*. | GumTree edit actions → rules matching; no type information needed. Shows that dynamic languages make extract/inline the hard cases. |
| [GumTree](https://github.com/GumTreeDiff/gumtree) | C, Java, JS, Python, R, **Ruby** | AST edit script: insert, delete, update, move | n/a | Two-phase matching (below) |
| [Mergiraf](https://mergiraf.org/architecture.html) | many, via tree-sitter | merge, not diff | n/a | A Rust implementation of GumTree matching on tree-sitter ASTs: top-down on unique isomorphic subtrees (largest first), bottom-up when enough descendants match; falls back to line-based when parsing fails |
| [difftastic](https://difftastic.wilfred.me.uk/diffing.html) | many, via tree-sitter | display diff | n/a | Dijkstra over (left, right) node pairs. [No move detection](https://difftastic.wilfred.me.uk/tricky_cases.html): reorders show up as changes. Falls back to line-based on parse errors. Comments get substring and indentation-insensitive matching. |
| [SemanticDiff](https://semanticdiff.com/docs/invariances/) (commercial) | ~15 | hides formatting and "invariances"; [renames only](https://semanticdiff.com/docs/understand-diff/refactoring-grouping/) as refactoring | n/a | Per-language invariance lists (redundant parens, numeric base, string escapes). Caution: it treats JS object-key reordering as invariant, which isn't strictly true. |

**Techniques for shapr:**
- **Format-only means equal ASTs with locations stripped,** and never token-equal. Ruby is full of traps: `do…end` vs `{}` bind differently on a parenless call; dropping parens changes `puts (a)*b`; hash key order is observable. Prism node equality without locations is the test.
- **Rename:** compare old and new token streams with each identifier canonicalized. Graded by how consistently it applies: all occurrences in scope vs some.
- **Move / move_with_changes / extract:** body similarity using RefDiff's weighted Jaccard, plus the GumTree matcher to locate the edits inside a moved body. Expose the similarity value; don't hide it behind a label.
- **Inline is the inverse of extract** and shows up in the bug data (§4). It needs its own kind.

## 3. Tangled changes and untangling

| Approach | Signals | Result | Caveat |
|---|---|---|---|
| [Herzig & Zeller 2013](https://www.st.cs.uni-saarland.de/publications/files/herzig-msr-2013.pdf) | "Confidence voters": file distance, call graph, data dependency, change coupling, then multilevel clustering | 7–20% of bug-fix commits tangled; ≥16% of files misattributed to bugs; untangling precision 0.67–0.93 for 2-concern blobs, dropping about 14 points at 3 | Java, artificially tangled evaluation |
| [Dias et al. 2015, EpiceaUntangler](https://arxiv.org/abs/1502.06757) | IDE-recorded edits; random forest over pairwise voters (time apart, intervening edits, same class), then agglomerative clustering | Median 91.5% / mean 75% success, 6 developers over 2 weeks | Needs IDE edit history; developers found reviewing the clusters "a bit painful" |
| [Flexeme 2020](https://partachi.com/Flexeme/) | Multiversion PDG with name flows; graph kernel + agglomerative clustering | 0.81 accuracy on C# | C# only |
| [SmartCommit 2021](https://2021.esec-fse.org/details/fse-2021-papers/20/SmartCommit-A-Graph-Based-Interactive-Assistant-for-Activity-Oriented-Commits) | Graph partitioning over changes, interactive | Median 71–84% on 3,000 synthetic composites; 83 engineers over 9 months in the field | Synthetic ground truth |
| [Atomizer 2026](https://arxiv.org/abs/2601.01233) | Tree-sitter minimal change subgraphs → LLM intent labels (what/how/why) → greedy grouping → reviewer agent, up to 3 rounds | C# 57% vs 50% for HD-GNN and 33% for Flexeme; about $0.06 per commit | Synthetic merges; not deterministic even at temperature 0 |
| [AtomicCommitBench 2026](https://arxiv.org/abs/2607.03332) | Coding agents partition hunks into replayable commits | 800 real Python episodes: replay validity ≥0.99, but grouping ARI only 0.03–0.46. **Synthetic composites scored 0.33 ARI higher than real ones.** Typical errors: lumping same-file hunks together, and "support hunks" drifting into the wrong commit. Dependency hints added 0.05–0.08 ARI. | The most realistic benchmark found |

**For the decomposer:** the unit is the hunk, plus dependency edges (def/use, same method, same file). First split along shapr's labels (Tidy First: structure before behavior), and assign each support hunk (import, test, fixture) to the commit it serves. Require every proposed commit to apply cleanly in order, and ideally to pass tests. Present the result as an editable draft: interactive tools are the ones developers actually adopted.

## 4. Are refactorings safe?

| Finding | Source | Strength |
|---|---|---|
| 103 Apache projects, controlling for change size: refactoring actions are linked to bug-inducing changes, confirming the 2012 study (which flagged hierarchy refactorings like pull-up). Manual review of 384 fix-inducing changes "partially debunked" this: many were really co-occurring changes, but a solid share were refactorings that did induce fixes (patterns: unplanned refactors, reverts). | [Di Penta, Bavota, Zampetti 2020](https://arxiv.org/abs/2009.11685) | Moderate |
| 12 Java projects, 27k refactorings: refactored code is *less* bug-prone than unrefactored smelly code (odds ratio 0.29 single, 0.69 composite). Bugs most often followed Extract Method (54%) and Inline Method (25%). The paper contradicts itself on floss vs root-canal refactoring, and the root-canal sample is small (23 events). | [Ferreira et al. 2025](https://arxiv.org/abs/2505.08005) | Weak–moderate |
| OpenStack, 5.5k vs 5.5k reviews: refactoring reviews take longer (median about 2.2x), need more revisions (3 vs 2) and get more comments (9 vs 6), but they are also much bigger (median churn 114 vs 11 lines), so size confounds the comparison. Reviewers warn that mixing in refactoring "overshadows errors" and ask for it to be separated. | [AlOmar et al. 2022](https://arxiv.org/abs/2203.14404) | Moderate (qualitative part is strong) |
| Refactoring-aware diff (RAID): median lines to review drop 14.5 → 2 for moves and 113 → 55 for extracts. 26% of PRs contained refactorings; Change Signature was 73% of them. 13% of move/extract detections were false positives. | [Brito & Valente 2021](https://arxiv.org/abs/2103.11453) | Small study (8 developers, Go) |

**Net:** no source shows that "pure refactor" PRs are reviewed faster or better, because nobody has measured it. The evidence supports *separating* structure from behavior for reviewability, and it doesn't support treating structure as risk-free.

## 5. Reputation, hotspots, ownership, SZZ

| Finding | Source |
|---|---|
| Ownership (count of low-expertise contributors, top owner's share) correlates with pre- and post-release failures in Windows Vista and 7. | [Bird et al. 2011](https://www.microsoft.com/en-us/research/?p=161465) |
| Replicated at Microsoft (file level P 0.74 / R 0.38; directory level P 0.76 / R 0.60). An OSS replication (Foucault) cast doubt on whether the effect generalizes. | [Greiler et al. 2015](https://www.microsoft.com/en-us/research/publication/code-ownership-and-software-quality-a-replication-study/) |
| Hotspots = high change frequency × low code health. Prioritization adds change coupling and multi-team contact. The formula isn't published. | [CodeScene docs](https://codescene.io/docs/guides/technical/hotspots.html) |
| Open-source git-log analyses: revisions, churn, coupling, age, entity-ownership, main-dev, fragmentation (GPL-3.0). Reimplementing these is cheap. | [code-maat](https://github.com/adamtornhill/code-maat) |
| **SZZ noise:** median 42% of Jira BUG issues are mislabeled; only about half of SZZ "bug-fix" commits are real fixes; SZZ misses about 1/5 of real ones. Recommendations: ignore whitespace, comment, test, doc and refactoring changes when blaming; assign defects to releases via the commit graph, not a time window. | [Herbold et al. 2022](https://arxiv.org/abs/1911.08938) |
| Developer-informed oracle of 1,115 fixes in 8 languages, Ruby included. Precision / recall: B-SZZ 0.39 / 0.69, AG-SZZ 0.45 / 0.60, **R-SZZ (most recent candidate) 0.66 / 0.57**, PyDriller 0.39 / 0.67, SZZ Unleashed 0.09 / 0.72. All variants failed on 311 fixes; typical causes: fixes that only add lines (null checks), reverts, history rewrites. | [Rosa et al. 2021](https://arxiv.org/abs/2102.03300) |

**Person-level scores, ethics and confounders:** ownership effects appear at the file and directory level, and even there they don't replicate everywhere. A "bug-introducer" score per person inherits SZZ's ~50% label noise and is confounded by who gets the hard files: hotspot owners write more bug-linked code because they write more code. Use experience as a *feature of a change* (the author's prior commits to this file: Kamei's EXP/SEXP) and keep people out of leaderboards. Attribute against a per-file baseline, as planned.

## 6. Comment changes

| Finding | Source | Strength |
|---|---|---|
| Changes that leave code and comments inconsistent are about 1.5x more likely to lead to a bug-introducing commit; the effect is strongest right after the inconsistency appears. Inconsistencies were detected with GPT-3.5. | [arXiv 2409.10781](https://arxiv.org/abs/2409.10781) | Weak (LLM-labeled, abstract-level) |
| Refactoring tools match comments that were only reformatted and offer `--ignore-formatting`. | [RefactoringMiner](https://github.com/tsantalis/RefactoringMiner) | n/a |

The direct evidence is thin. No source we found measures the risk of comment-only changes or comment bloat. Practical reading: comment-only hunks are low risk. The risky case is the opposite one, a logic change inside a construct whose doc comment *didn't* change: a cheap, deterministic staleness hint for the review agent.

## 7. AI code review tools

| Tool or study | What keeps it high-signal | Numbers |
|---|---|---|
| [AutoCommenter (Google)](https://arxiv.org/abs/2405.13565) | Confidence threshold per category instead of a global 0.98; drop comments on lines the author didn't change; regex suppression of non-actionable categories (deployable without retraining); track usefulness continuously | Useful rate 54% → 66% after suppressing 17 categories → >80%. About 40% of comments estimated resolved. A/B test: no change in review duration. Offline evaluation didn't predict production results. |
| [PR-Agent / Qodo in industry](https://arxiv.org/abs/2412.18531) ([repo](https://github.com/qodo-ai/pr-agent), MIT) | — | 74% of 1.4k comments resolved, 21% "won't fix". Mean PR closure time 5.9 h → 8.3 h. Complaints: out-of-scope suggestions, false findings, repeat reviews on each revision. |
| [Qodo noise controls](https://docs.qodo.ai/code-review/reduce-review-noise.md) | Route by severity (High inline, Medium in the summary, Low hidden); cap visible findings per severity group | — |

**Lessons for the review agent:** comment only on changed lines; set a threshold per category; keep a suppression list grown from rejected comments (the planned learning loop); rank by severity and cap Medium/Low; don't re-review unchanged hunks on each revision; measure accept/resolve rate per category.

## Recommendations for shapr

| Idea | Fits | Effort | Evidence |
|---|---|---|---|
| Emit `review_lines`: added + deleted lines after discounting format, comment, rename and the unchanged body of moves | shapr | S | Moderate (RAID; LA is the top JIT feature) |
| Emit per-file LA, LD, LT and comment lines separately, plus PR-level NF, ND, NS, entropy | shapr → risk model | S | Strong (Kamei, Zeng) |
| Format-only = prism AST equality with locations stripped; no token heuristics | shapr | S | Strong (Ruby semantics) |
| Add `inline` kind (inverse of extract) | shapr | S | Moderate (RefDiff/RM catalogs; bug data) |
| Treat signature changes as `logic` unless only names changed; consider a `signature` kind later | shapr | S | Moderate (73% of refactorings in RAID were Change Signature) |
| Graded confidence derived from evidence (similarity score, rename consistency, parse status), with the evidence values in the output | shapr | M | Strong (every detector uses calibrated thresholds) |
| RefDiff-style TF-IDF weighted-Jaccard body similarity for move/extract/inline | shapr | M | Moderate (Java/Go/Python results; untested on Ruby) |
| GumTree matching (Mergiraf's Rust version as reference) to find edits inside moved bodies | shapr | M–L | Moderate |
| Line-diff fallback on parse error, with a `parsed: false` flag and capped confidence | shapr | S | Strong (difftastic, Mergiraf) |
| `--ignore-revs`-style output: commits/lines that are structure-only, for refactoring-aware blame and SZZ | shapr → retrospective signals | S | Strong (Herbold, Rosa) |
| Baseline risk model: LR on lines added only; ship Kamei-14 LR only if it beats that | risk model | S | Strong |
| Effort-aware evaluation (defects caught per % LOC reviewed), not accuracy | risk model | S | Strong |
| Labels: reverts and hotfixes first; R-SZZ with cosmetic/structure hunks filtered second | risk model | M | Moderate |
| Retrain on a rolling window; version the model in each verdict | risk model | S | Moderate |
| File-level ownership (minor-contributor count, top-owner share) and hotspot = churn × complexity | risk model | S | Moderate (replication mixed) |
| Split along shapr labels first, then by dependency, then LLM intent; every proposed commit must replay | decomposer | M | Moderate (AtomicCommitBench, Atomizer) |
| Present splits as an editable draft; log edits as fixtures | decomposer | M | Moderate (EpiceaUntangler, SmartCommit) |
| Flag logic changes under unchanged doc comments | review agent | S | Weak |
| Comments only on changed lines; per-category thresholds; suppression list fed by rejections; severity caps | review agent | S | Strong (AutoCommenter) |
| Extracts and inlines still get a look even in "pure structure" PRs | review agent | S | Moderate |

## Skip / low value

| Idea | Why skip |
|---|---|
| Deep JIT models (DeepJIT, CC2Vec) | No better than LA + LR; CC2Vec's results relied on test-set leakage. Revisit only with a large corpus. |
| Running RefactoringMiner/RefDiff as a dependency | JVM, no Ruby support; the techniques port, the tools don't. |
| Issue-tracker-based bug labels in v1 | About half are wrong without manual validation, and it needs infra. |
| Person leaderboards and "bug-introducer" scores | Label noise plus confounding by file difficulty; ownership effects are file-level and don't replicate everywhere. |
| IDE-history untangling (EpiceaUntangler) | Requires edit recordings we don't have. |
| Claiming "pure structure = safe" | Contradicted by the refactoring-bug evidence (§4). |
| Treating all reorderings as invariant (SemanticDiff-style) | Order is semantic in Ruby hashes, JS object iteration, and Python kwargs in some APIs. |
