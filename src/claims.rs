//! What the messages around a change claim about it: commit messages in the
//! range and a description (a PR body). Claims are read deterministically and
//! reported beside the code's facts, never folded into a hunk's kind or
//! facets: shapr reads the code first and doesn't take the message's word.

use crate::change::Category as FileCategory;
use crate::classify::{Facet, Kind};
use crate::report::{HunkRecord, Verdict};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// A message and where it came from.
pub struct Message {
    pub source: Source,
    /// The commit id, for a commit message.
    pub commit: Option<String>,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Commit,
    /// The description given with `--message`.
    Message,
    /// A GitHub pull request's title and description.
    PullRequest,
}

/// What a claim says the change is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Fix,
    Feature,
    Refactor,
    Perf,
    Test,
    Docs,
    /// Formatting and lint fixes: the change should hold only reformatted
    /// or respelled code.
    Style,
    Chore,
    Revert,
}

impl Category {
    pub const ALL: [Category; 9] = [
        Category::Fix,
        Category::Feature,
        Category::Refactor,
        Category::Perf,
        Category::Test,
        Category::Docs,
        Category::Style,
        Category::Chore,
        Category::Revert,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Fix => "fix",
            Category::Feature => "feature",
            Category::Refactor => "refactor",
            Category::Perf => "perf",
            Category::Test => "test",
            Category::Docs => "docs",
            Category::Style => "style",
            Category::Chore => "chore",
            Category::Revert => "revert",
        }
    }

    /// Claims the change alters behavior.
    fn behavior(self) -> bool {
        matches!(self, Category::Fix | Category::Feature | Category::Perf)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HintKind {
    /// A conventional-commit type (`fix:`, `feat(api):`).
    Type,
    /// A word naming a purpose (`fix`, `regression`, `add`, `refactor`).
    Keyword,
    /// `Revert "subject"`, or `This reverts commit <id>`.
    Revert,
    /// An issue: `PROJ-34`, an issue URL, or a `#12` GitHub says is one.
    Issue,
    /// A pull request: a pull URL, or a `#12` GitHub says is one.
    PullRequest,
    /// `#12`: an issue or a pull request, which share GitHub's numbering.
    Reference,
    /// `type!:` or a `BREAKING CHANGE:` footer.
    Breaking,
    /// "No behavior change", "no functional change", "NFC".
    NoBehaviorChange,
    /// "Test-only", "tests only": the change is to test code.
    TestOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Hint {
    pub hint: HintKind,
    pub value: String,
    pub category: Option<Category>,
    pub source: Source,
    pub commit: Option<String>,
    pub field: Field,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Subject,
    Body,
}

/// A claim the code doesn't bear out, stated as a count of what it shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Conflict {
    pub claim: Category,
    pub finding: Finding,
    pub count: usize,
    /// The ids of the hunks behind the finding.
    pub hunks: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Finding {
    /// Hunks shapr compared and couldn't prove structural.
    BehaviorHunks,
    /// The same, outside test code.
    NonTestBehaviorHunks,
    /// Every hunk is structure.
    NoBehaviorHunks,
    /// Hunks in code or tests shapr couldn't prove structural, against a
    /// style claim.
    LogicHunks,
    /// No hunk holds a reorder or move part, against a reorder claim.
    NoReorderParts,
}

#[derive(Serialize)]
pub struct Claims {
    pub messages: usize,
    pub hints: Vec<Hint>,
    pub experimental: Experimental,
}

/// Judgements drawn from the hints by unmeasured keyword rules: unstable
/// until measured against labelled changes.
#[derive(Serialize)]
pub struct Experimental {
    /// Messages carrying a hint of each category, every category present.
    pub categories: BTreeMap<&'static str, usize>,
    pub conflicts: Vec<Conflict>,
}

/// What the code shows, for checking claims against it.
struct CodeFacts {
    /// Every hunk is structure (and there is at least one).
    structure: bool,
    /// The hunks in the verdict (structure, behavior or mixed): hunks shapr
    /// couldn't compare are unknown, not evidence.
    hunks: Vec<HunkFacts>,
}

struct HunkFacts {
    id: usize,
    /// Its verdict is behavior or mixed.
    behavior: bool,
    /// In test code: a test file, or a residual with the `test` facet.
    test: bool,
    /// In a code or test file, not config or a manifest.
    code: bool,
    /// Holds a reorder or move part.
    reorder: bool,
}

impl CodeFacts {
    fn of(verdict: Verdict, hunks: &[HunkRecord]) -> CodeFacts {
        CodeFacts {
            structure: verdict == Verdict::Structure,
            hunks: hunks
                .iter()
                .filter(|h| h.verdict.counted())
                .map(|h| HunkFacts {
                    id: h.id,
                    behavior: matches!(h.verdict, Verdict::Behavior | Verdict::Mixed),
                    test: h.category == FileCategory::Test
                        || h.parts.iter().any(|p| p.facets.contains(&Facet::Test)),
                    code: matches!(h.category, FileCategory::Code | FileCategory::Test),
                    reorder: h
                        .parts
                        .iter()
                        .any(|p| matches!(p.kind, Kind::Reorder | Kind::Move)),
                })
                .collect(),
        }
    }
}

impl Claims {
    pub fn read(messages: &[Message]) -> Claims {
        let mut hints = Vec::new();
        let mut categories: BTreeMap<&'static str, usize> =
            Category::ALL.iter().map(|c| (c.as_str(), 0)).collect();
        let subjects: BTreeSet<String> = messages
            .iter()
            .filter(|m| m.source == Source::Commit)
            .map(|m| title_key(&m.text))
            .collect();
        for message in messages {
            let mut found = hints_in(message);
            // A pull request titled after its commit says it once.
            if message.source != Source::Commit && subjects.contains(&title_key(&message.text)) {
                found.retain(|h| h.field == Field::Body);
            }
            let claimed: BTreeSet<Category> = found.iter().filter_map(|h| h.category).collect();
            for category in claimed {
                *categories.entry(category.as_str()).or_default() += 1;
            }
            hints.extend(found);
        }
        Claims {
            messages: messages.len(),
            hints,
            experimental: Experimental {
                categories,
                conflicts: Vec::new(),
            },
        }
    }

    /// The numbers of the `#N` references, each once, in message order.
    pub fn references(&self) -> Vec<u64> {
        let mut numbers: Vec<u64> = Vec::new();
        for hint in self.hints.iter().filter(|h| h.hint == HintKind::Reference) {
            if let Some(n) = reference_number(&hint.value)
                && !numbers.contains(&n)
            {
                numbers.push(n);
            }
        }
        numbers
    }

    /// Retypes each `#N` reference whose target is known: `kinds` maps a
    /// number to `Issue` or `PullRequest`.
    pub fn resolve(&mut self, kinds: &BTreeMap<u64, HintKind>) {
        for hint in self
            .hints
            .iter_mut()
            .filter(|h| h.hint == HintKind::Reference)
        {
            if let Some(kind) = reference_number(&hint.value).and_then(|n| kinds.get(&n)) {
                hint.hint = *kind;
            }
        }
    }

    /// Records each claim the code doesn't bear out. A purpose claim (refactor,
    /// docs, test, style) conflicts only when no message also claims a behavior
    /// change.
    /// The change's VERDICT and HUNKS are the code's side.
    pub fn check(&mut self, verdict: Verdict, hunks: &[HunkRecord]) {
        let code = &CodeFacts::of(verdict, hunks);
        let claimed = |c: Category| self.experimental.categories[c.as_str()] > 0;
        let behavior = Category::ALL.iter().any(|&c| c.behavior() && claimed(c));
        let reorder = self
            .hints
            .iter()
            .any(|h| h.hint == HintKind::Keyword && REORDER.contains(&h.value.as_str()));
        let ids = |keep: &dyn Fn(&HunkFacts) -> bool| -> Vec<usize> {
            code.hunks
                .iter()
                .filter(|h| keep(h))
                .map(|h| h.id)
                .collect()
        };
        let mut conflicts = Vec::new();
        let mut conflict = |claim, finding, hunks: Vec<usize>| {
            conflicts.push(Conflict {
                claim,
                finding,
                count: hunks.len(),
                hunks,
            })
        };
        if !behavior {
            // A test claim says the tests changed: only code outside them
            // can contradict a purpose claim beside it.
            let tests = claimed(Category::Test);
            let outside = |h: &HunkFacts| !(tests && h.test);
            let finding = match tests {
                true => Finding::NonTestBehaviorHunks,
                false => Finding::BehaviorHunks,
            };
            let unproven = ids(&|h| h.behavior && outside(h));
            for claim in [Category::Refactor, Category::Docs] {
                if claimed(claim) && !unproven.is_empty() {
                    conflict(claim, finding, unproven.clone());
                }
            }
            // A linter's config and manifest edits come with its fixes.
            let logic = ids(&|h| h.behavior && h.code && outside(h));
            if claimed(Category::Style) && !logic.is_empty() {
                conflict(Category::Style, Finding::LogicHunks, logic);
            }
            if reorder && !code.hunks.is_empty() && !code.hunks.iter().any(|h| h.reorder) {
                conflict(Category::Refactor, Finding::NoReorderParts, ids(&|_| true));
            }
            let outside_tests = ids(&|h| h.behavior && !h.test);
            if tests && !outside_tests.is_empty() {
                conflict(Category::Test, Finding::NonTestBehaviorHunks, outside_tests);
            }
        }
        if code.structure {
            for claim in Category::ALL.into_iter().filter(|&c| c.behavior()) {
                if claimed(claim) {
                    conflict(claim, Finding::NoBehaviorHunks, Vec::new());
                }
            }
        }
        self.experimental.conflicts = conflicts;
    }
}

/// Conventional-commit types and what each claims.
const TYPES: &[(&str, Category)] = &[
    ("fix", Category::Fix),
    ("feat", Category::Feature),
    ("refactor", Category::Refactor),
    ("style", Category::Style),
    ("perf", Category::Perf),
    ("test", Category::Test),
    ("docs", Category::Docs),
    ("chore", Category::Chore),
    ("build", Category::Chore),
    ("ci", Category::Chore),
    ("revert", Category::Revert),
];

/// Subject words that name a purpose.
const KEYWORDS: &[(&str, Category)] = &[
    ("fix", Category::Fix),
    ("fixes", Category::Fix),
    ("fixed", Category::Fix),
    ("fixing", Category::Fix),
    ("bug", Category::Fix),
    ("bugfix", Category::Fix),
    ("regression", Category::Fix),
    ("crash", Category::Fix),
    ("crashes", Category::Fix),
    ("hotfix", Category::Fix),
    ("broken", Category::Fix),
    ("add", Category::Feature),
    ("adds", Category::Feature),
    ("added", Category::Feature),
    ("introduce", Category::Feature),
    ("introduces", Category::Feature),
    ("support", Category::Feature),
    ("supports", Category::Feature),
    ("implement", Category::Feature),
    ("implements", Category::Feature),
    ("allow", Category::Feature),
    ("allows", Category::Feature),
    ("refactor", Category::Refactor),
    ("refactors", Category::Refactor),
    ("refactoring", Category::Refactor),
    ("rename", Category::Refactor),
    ("renames", Category::Refactor),
    ("move", Category::Refactor),
    ("moves", Category::Refactor),
    ("extract", Category::Refactor),
    ("extracts", Category::Refactor),
    ("tidy", Category::Refactor),
    ("cleanup", Category::Refactor),
    ("simplify", Category::Refactor),
    ("simplifies", Category::Refactor),
    ("restructure", Category::Refactor),
    ("reorder", Category::Refactor),
    ("reordered", Category::Refactor),
    ("reordering", Category::Refactor),
    ("alphabetize", Category::Refactor),
    ("dry", Category::Refactor),
    ("perf", Category::Perf),
    ("performance", Category::Perf),
    ("faster", Category::Perf),
    ("optimize", Category::Perf),
    ("optimise", Category::Perf),
    ("test", Category::Test),
    ("tests", Category::Test),
    ("spec", Category::Test),
    ("specs", Category::Test),
    ("coverage", Category::Test),
    ("typo", Category::Docs),
    ("chore", Category::Chore),
    ("bump", Category::Chore),
    ("bumps", Category::Chore),
    ("deps", Category::Chore),
    ("dependency", Category::Chore),
    ("dependencies", Category::Chore),
    ("upgrade", Category::Chore),
    ("release", Category::Chore),
    ("rubocop", Category::Style),
    ("autocorrect", Category::Style),
    ("lint", Category::Style),
    ("linting", Category::Style),
    ("reformat", Category::Style),
    ("revert", Category::Revert),
    ("reverts", Category::Revert),
];

/// Phrases claiming behavior stays the same, matched lowercase with hyphens
/// as spaces.
const NO_BEHAVIOR_CHANGE: &[&str] = &[
    "no behavior change",
    "no behaviour change",
    "no functional change",
    "no logic change",
    "behavior preserving",
    "behaviour preserving",
    "pure refactor",
];

/// Keywords claiming a reorder, checked against the reorder and move parts.
const REORDER: &[&str] = &["reorder", "reordered", "reordering", "alphabetize"];

/// Words that fix what a style word in the same subject names ("Fix lint
/// errors"): they claim style, not a bug fix.
const STYLE_FIXES: &[&str] = &["fix", "fixes", "fixed", "fixing"];

/// Phrases saying the change is to test code, matched as `NO_BEHAVIOR_CHANGE`.
const TEST_ONLY: &[&str] = &["test only", "tests only", "spec only", "specs only"];

/// Words before an issue reference that say it was a bug.
const FIXING: &[&str] = &["fix", "fixes", "fixed"];

/// Words that claim a fix when they open a body line ("Fixes the flake"):
/// a summary, where elsewhere in a body they are a passing mention.
const OPENING_FIXES: &[&str] = &["fixes", "fixed"];

/// Look like `KEY-12` issue keys but are standards.
const NOT_ISSUE_KEYS: &[&str] = &["UTF", "SHA", "ISO", "RFC", "PEP", "MD", "CVE", "EUI"];

/// Hints in one message, in order, each once. The first non-empty line is
/// the subject. Purpose words count only there (a body says too much in
/// passing); a body adds reverts, issues, breaking footers and phrases.
fn hints_in(message: &Message) -> Vec<Hint> {
    let subject = subject(&message.text);
    let body: Vec<&str> = message
        .text
        .lines()
        .skip_while(|l| l.trim().is_empty())
        .skip(1)
        .collect();
    let body = body.join("\n");
    let mut hints: Vec<Hint> = Vec::new();
    let mut add = |hint: HintKind, value: &str, category: Option<Category>, field: Field| {
        let new = Hint {
            hint,
            value: value.to_string(),
            category,
            source: message.source,
            commit: message.commit.clone(),
            field,
        };
        if !hints
            .iter()
            .any(|h| h.hint == hint && h.value == new.value && h.field == field)
        {
            hints.push(new);
        }
    };

    // A revert quotes the reverted subject; its words are that commit's claims.
    if let Some(quoted) = subject
        .strip_prefix("Revert \"")
        .map(|s| s.strip_suffix('"').unwrap_or(s))
    {
        add(
            HintKind::Revert,
            quoted,
            Some(Category::Revert),
            Field::Subject,
        );
    } else {
        let mut words = subject;
        if let Some((kind, breaking, rest)) = conventional(subject) {
            let category = TYPES.iter().find(|(t, _)| *t == kind).map(|(_, c)| *c);
            add(HintKind::Type, &kind, category, Field::Subject);
            if breaking {
                add(HintKind::Breaking, "!", None, Field::Subject);
            }
            words = rest;
        }
        let words = purpose_words(words);
        let style = words
            .iter()
            .any(|w| KEYWORDS.contains(&(w.as_str(), Category::Style)));
        for word in words {
            if let Some((_, category)) = KEYWORDS.iter().find(|(k, _)| *k == word) {
                let category = match style && STYLE_FIXES.contains(&word.as_str()) {
                    true => Category::Style,
                    false => *category,
                };
                add(HintKind::Keyword, &word, Some(category), Field::Subject);
            }
        }
        // "moves to x.rs, unchanged": the closing clause; elsewhere the word
        // names data ("skip a large, unchanged repo").
        if normalize(subject)
            .trim_end_matches('.')
            .ends_with(", unchanged")
        {
            add(
                HintKind::NoBehaviorChange,
                "unchanged",
                Some(Category::Refactor),
                Field::Subject,
            );
        }
    }
    for line in body.lines() {
        if let Some(rest) = line.trim().strip_prefix("This reverts commit ") {
            let id: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
            if !id.is_empty() {
                add(HintKind::Revert, &id, Some(Category::Revert), Field::Body);
            }
        }
        if line.starts_with("BREAKING CHANGE:") || line.starts_with("BREAKING-CHANGE:") {
            add(HintKind::Breaking, "BREAKING CHANGE", None, Field::Body);
        }
        if let Some(word) = opening_fix(line) {
            add(
                HintKind::Keyword,
                &word,
                Some(fix_category(line)),
                Field::Body,
            );
        }
    }
    for (field, text) in [(Field::Subject, subject), (Field::Body, body.as_str())] {
        let mut previous = String::new();
        for word in text.split_whitespace() {
            let token = word.trim_matches(|c: char| ",.;:()[]{}<>!?'\"".contains(c));
            if let Some((kind, value)) = issue(token) {
                let category = FIXING.contains(&previous.as_str()).then_some(Category::Fix);
                add(kind, &value, category, field);
            }
            previous = token.to_ascii_lowercase();
        }
        let normal = normalize(text);
        for phrase in NO_BEHAVIOR_CHANGE {
            if normal.contains(phrase) {
                add(
                    HintKind::NoBehaviorChange,
                    phrase,
                    Some(Category::Refactor),
                    field,
                );
            }
        }
        for phrase in TEST_ONLY {
            if normal.contains(phrase) && !normal.contains(&format!("not {phrase}")) {
                add(HintKind::TestOnly, phrase, Some(Category::Test), field);
            }
        }
        if tokens(text).any(|w| w == "NFC") {
            add(
                HintKind::NoBehaviorChange,
                "nfc",
                Some(Category::Refactor),
                field,
            );
        }
    }
    hints
}

/// `fixes` or `fixed` opening a body line, after any list marker, and
/// followed by what it fixes, unless that is an issue reference, which
/// claims the fix itself.
fn opening_fix(line: &str) -> Option<String> {
    let line = line.trim_start();
    let line = ["- ", "* ", "+ "]
        .iter()
        .find_map(|m| line.strip_prefix(m))
        .unwrap_or(line);
    let mut words = line.split_whitespace();
    let first = words.next()?;
    let word = first
        .trim_end_matches(|c: char| ",.;:!".contains(c))
        .to_ascii_lowercase();
    // A template's "Fixes # (issue)" or a bare "Fixes:" says nothing yet.
    let what = words.next().filter(|w| !w.starts_with(['#', '<']))?;
    let reference = issue(what.trim_matches(|c: char| ",.;:()".contains(c))).is_some();
    (OPENING_FIXES.contains(&word.as_str()) && !reference).then_some(word)
}

/// A fix claim's category: `style` beside a style word ("Fixes rubocop
/// offenses"), else `fix`.
fn fix_category(text: &str) -> Category {
    let style = purpose_words(text)
        .iter()
        .any(|w| KEYWORDS.contains(&(w.as_str(), Category::Style)));
    match style {
        true => Category::Style,
        false => Category::Fix,
    }
}

/// The first non-empty line.
fn subject(text: &str) -> &str {
    text.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .trim()
}

/// The subject as a title repeats it: case, spacing and closing punctuation
/// aside.
fn title_key(text: &str) -> String {
    let words: Vec<&str> = subject(text).split_whitespace().collect();
    let key = words.join(" ").to_lowercase();
    key.trim_end_matches(|c: char| c.is_ascii_punctuation())
        .to_string()
}

/// `type(scope)!: rest` as (type, breaking, rest), for a known type.
fn conventional(subject: &str) -> Option<(String, bool, &str)> {
    let (head, rest) = subject.split_once(':')?;
    let (head, breaking) = match head.strip_suffix('!') {
        Some(h) => (h, true),
        None => (head, false),
    };
    let kind = match head.split_once('(') {
        Some((kind, scope)) if scope.ends_with(')') => kind,
        Some(_) => return None,
        None => head,
    };
    let kind = kind.to_ascii_lowercase();
    TYPES
        .iter()
        .any(|(t, _)| *t == kind)
        .then_some((kind, breaking, rest))
}

/// The lowercase words that can name a purpose: not inside a path or an
/// identifier (`spec/support`, `spec_helper`, `Foo::move`), and not denied by
/// a `no` or `not a` before them ("an edit, not a rename").
fn purpose_words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut previous: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let code = ["/", "_", "::", "#"].iter().any(|c| word.contains(c));
        for token in tokens(word) {
            let token = token.to_ascii_lowercase();
            let denied = match previous.as_slice() {
                [.., no] if no == "no" => true,
                [.., not, a] if not == "not" && (a == "a" || a == "an") => true,
                _ => false,
            };
            if !code && !denied {
                out.push(token.clone());
            }
            previous.push(token);
        }
    }
    out
}

fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
}

fn normalize(text: &str) -> String {
    text.to_lowercase()
        .replace('-', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `#12`, `KEY-12`, or an issue or pull URL (as `#12`), and what it refers to.
fn issue(token: &str) -> Option<(HintKind, String)> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(n) = token.strip_prefix('#') {
        return digits(n).then(|| (HintKind::Reference, token.to_string()));
    }
    for (marker, kind) in [
        ("/issues/", HintKind::Issue),
        ("/pull/", HintKind::PullRequest),
    ] {
        if let Some((_, n)) = token.rsplit_once(marker) {
            return digits(n).then(|| (kind, format!("#{n}")));
        }
    }
    let (key, n) = token.split_once('-')?;
    let key_ok = key.len() >= 2
        && key.starts_with(|c: char| c.is_ascii_uppercase())
        && key
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        && !NOT_ISSUE_KEYS.contains(&key);
    (key_ok && digits(n)).then(|| (HintKind::Issue, token.to_string()))
}

fn reference_number(value: &str) -> Option<u64> {
    value.strip_prefix('#')?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hints(text: &str) -> Vec<(HintKind, String, Option<Category>)> {
        let message = Message {
            source: Source::Message,
            commit: None,
            text: text.to_string(),
        };
        hints_in(&message)
            .into_iter()
            .map(|h| (h.hint, h.value, h.category))
            .collect()
    }

    #[test]
    fn conventional_types_need_a_known_type() {
        assert_eq!(
            conventional("fix(api)!: x"),
            Some(("fix".to_string(), true, " x"))
        );
        assert_eq!(conventional("wip: x"), None);
        assert_eq!(conventional("Note: x"), None);
    }

    #[test]
    fn issue_keys_skip_standards_and_bare_words() {
        assert_eq!(
            issue("https://github.com/o/r/pull/9"),
            Some((HintKind::PullRequest, "#9".into()))
        );
        assert_eq!(
            issue("https://github.com/o/r/issues/9"),
            Some((HintKind::Issue, "#9".into()))
        );
        assert_eq!(issue("#12"), Some((HintKind::Reference, "#12".into())));
        assert_eq!(issue("PROJ-34"), Some((HintKind::Issue, "PROJ-34".into())));
        for no in ["UTF-8", "SHA-256", "#abc", "x-1", "A-1", "re-run"] {
            assert_eq!(issue(no), None, "{no}");
        }
    }

    #[test]
    fn paths_identifiers_and_denials_claim_nothing() {
        for subject in [
            "Consolidate spec/support helpers",
            "Share spec_helper setup",
            "Treat a kept name as an edit, not a rename",
            "Say there was no regression",
        ] {
            let found = hints(subject);
            assert!(found.is_empty(), "{subject}: {found:?}");
        }
        assert_eq!(hints("Re-add the gate")[0].1, "add");
        assert_eq!(hints("Don't crash on empty input")[0].1, "crash");
    }

    #[test]
    fn test_only_is_a_test_claim_unless_denied() {
        assert_eq!(
            hints("Tidy\n\nTest-only.")[1],
            (HintKind::TestOnly, "test only".into(), Some(Category::Test))
        );
        assert!(hints("Tidy\n\nThis is not test-only.").len() == 1);
    }

    #[test]
    fn purpose_words_count_only_in_the_subject() {
        let found = hints("Speed up lookups\n\nThis fixes a bug and adds a test.");
        assert!(found.is_empty(), "{found:?}");
    }

    /// A file named in the subject says what it touched, not what the
    /// change is for (rspec-networking#29, rwr#18).
    #[test]
    fn docs_words_name_a_subject_and_only_the_docs_type_claims_docs() {
        for subject in [
            "README: say which EUI-64 addresses match",
            "Say which existing rules change meaning in the changelog",
            "Update the docs",
            "Documentation for the retry option",
        ] {
            let found = hints(subject);
            assert!(found.is_empty(), "{subject}: {found:?}");
        }
        assert_eq!(
            categories("docs: say which rules match"),
            [Some(Category::Docs)]
        );
        assert_eq!(
            categories("Fix a typo"),
            [Some(Category::Fix), Some(Category::Docs)]
        );
    }

    /// "Fixes the flake" opening a description (rq#36) says what the change
    /// is for; the same word mid-sentence is a passing mention.
    #[test]
    fn a_body_line_opening_with_fixes_or_fixed_claims_fix() {
        let fix = || (HintKind::Keyword, "fixes".to_string(), Some(Category::Fix));
        assert_eq!(
            hints("Lay the database down once\n\nFixes the flake in a test."),
            [fix()]
        );
        assert_eq!(hints("Lay it down once\n\n- Fixes the race."), [fix()]);
        assert_eq!(
            hints("Lay it down once\n\nFixed: openers raced."),
            [(HintKind::Keyword, "fixed".into(), Some(Category::Fix))]
        );
        assert_eq!(
            hints("Lay it down once\n\nFixes rubocop offenses."),
            [(HintKind::Keyword, "fixes".into(), Some(Category::Style))]
        );
        // A reference after it already claims the fix.
        assert_eq!(
            hints("Lay it down once\n\nFixes #12"),
            [(HintKind::Reference, "#12".into(), Some(Category::Fix))]
        );
        for body in [
            "This fixes a bug and adds a test.",
            "Prefixes each line with its number.",
            "## Fix",
            "Fix the gate first.",
            "fixes_for(x) returns them.",
            "Fixes # (issue)",
            "Fixes:",
            "Fixes <!-- the issue -->",
        ] {
            let found = hints(&format!("Speed up lookups\n\n{body}"));
            assert!(found.is_empty(), "{body}: {found:?}");
        }
    }

    fn categories(text: &str) -> Vec<Option<Category>> {
        hints(text).into_iter().map(|h| h.2).collect()
    }

    #[test]
    fn style_words_claim_style_and_their_fix_with_them() {
        let style = Some(Category::Style);
        assert_eq!(categories("Fix lint errors"), [style, style]);
        assert_eq!(categories("Rubocop fixes."), [style, style]);
        assert_eq!(categories("Apply a reviewed rubocop pass"), [style]);
        assert_eq!(categories("style: reformat the gate"), [style, style]);
        // "format" names output formats as often as code layout.
        assert!(hints("Format dates in the log").is_empty());
        assert_eq!(categories("Fix the gate"), [Some(Category::Fix)]);
    }

    #[test]
    fn reorder_and_dry_claim_refactor_but_sort_does_not() {
        let refactor = Some(Category::Refactor);
        assert_eq!(categories("Reorder methods in feature"), [refactor]);
        assert_eq!(categories("Alphabetize adapters"), [refactor]);
        assert_eq!(
            categories("DRY up error tests"),
            [refactor, Some(Category::Test)]
        );
        assert!(hints("Sort fields when printing").is_empty());
    }

    #[test]
    fn unchanged_claims_only_as_a_trailing_clause_of_the_subject() {
        assert_eq!(
            hints("cli: the sweep goes to cli/refs.rs, unchanged"),
            [(
                HintKind::NoBehaviorChange,
                "unchanged".into(),
                Some(Category::Refactor)
            )]
        );
        assert!(hints("Skip an unchanged file").is_empty());
        assert!(hints("Skip the walk on a large, unchanged repo").is_empty());
        assert!(hints("Speed up\n\nThe suite, unchanged.").is_empty());
    }

    #[test]
    fn nfc_is_a_word_not_a_substring() {
        assert_eq!(hints("Tidy (NFC)").len(), 2);
        assert_eq!(
            hints("Tidy\n\nNFC"),
            [
                (HintKind::Keyword, "tidy".into(), Some(Category::Refactor)),
                (
                    HintKind::NoBehaviorChange,
                    "nfc".into(),
                    Some(Category::Refactor)
                )
            ]
        );
        assert!(hints("Update NFCReader").is_empty());
    }

    #[test]
    fn a_title_repeating_a_subject_in_other_case_spacing_or_punctuation_claims_once() {
        let message = |source, text: &str| Message {
            source,
            commit: None,
            text: text.to_string(),
        };
        let claims = Claims::read(&[
            message(Source::Commit, "fix lint errors"),
            message(Source::PullRequest, "  fix LINT  errors.\n\nbody"),
        ]);
        assert_eq!(claims.experimental.categories["style"], 1);
    }
}
