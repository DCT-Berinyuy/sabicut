//! Transcript fixes, `fixes.toml` (schema v1): replacement rules for words the speech
//! model gets wrong, such as names. They are reapplied to the words on every run.

use std::collections::HashMap;
use std::ops::Range;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::validate::{Report, ValidationErrors};
use crate::words::{Transcript, Word};

/// The fixes.toml format version this build reads.
pub const FIXES_SCHEMA_VERSION: u32 = 1;

/// A rule may match at most this many words.
const MAX_FROM_WORDS: usize = 32;
/// Passes run after the limit to find the rules that keep undoing each other.
const CYCLE_PASSES: usize = 8;

/// Symbols that belong to a word even at its edges, as in "C++", "C#", "50%" or "@dct".
/// Other punctuation at the edges of a word (quotes, commas, «», ...) is kept in the
/// text but ignored for matching.
const WORD_SYMBOLS: [char; 10] = ['+', '#', '%', '&', '@', '$', '€', '£', '°', '_'];

/// # Sabicut transcript fixes (fixes.toml, schema v1)
///
/// Replacement rules for words the speech model gets wrong, such as names and brands.
/// They are applied whenever words.json is read, so they survive a new transcription.
/// A rule matches whole words, ignores capitals unless it says otherwise, and keeps
/// the punctuation around the words. Where rules overlap, the one that matches more
/// words wins. Rules are applied again until nothing changes, so applying them twice
/// gives the same result as applying them once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fixes {
    /// Format version. Always 1 for this schema.
    #[schemars(extend("const" = 1))]
    schema: u32,
    /// The replacement rules.
    #[serde(rename = "fix", default, skip_serializing_if = "Vec::is_empty")]
    rules: Vec<Fix>,
}

/// One replacement rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fix {
    /// The words to find, like "Supa base". Only whole words match, and punctuation
    /// around them is ignored. At most 32 words.
    #[schemars(pattern(r"\S"))]
    from: String,
    /// What to write instead, like "Supabase". If it has as many words as `from`, each
    /// word keeps its own timing. Otherwise the matched words become one word that runs
    /// from the start of the first to the end of the last.
    #[schemars(pattern(r"\S"))]
    to: String,
    /// Match capitals exactly. By default "supa base" also matches "Supa Base".
    #[serde(default, skip_serializing_if = "is_false")]
    case_sensitive: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Why a `fixes.toml` could not be read.
#[derive(Debug, thiserror::Error)]
pub enum FixesParseError {
    /// The file has no top-level `schema` key.
    #[error("missing `schema = 1` (the fixes.toml format version) at the top of the file")]
    MissingSchema,
    /// `schema` is present but is not a whole number.
    #[error("`schema` must be a whole number, like `schema = 1`")]
    SchemaNotInteger,
    /// The file uses a format version that this build cannot read.
    #[error(
        "fixes.toml schema {0} is not supported: this version of sabicut reads schema {FIXES_SCHEMA_VERSION}"
    )]
    UnsupportedSchema(i64),
    /// The text is not valid TOML, or does not have the shape of schema v1 fixes.
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
}

/// Why fixes could not be applied.
#[derive(Debug, thiserror::Error)]
pub enum FixError {
    /// The rules break schema v1. Lists every problem, one per line.
    #[error("{0}")]
    Invalid(ValidationErrors),
    /// The rules keep changing the words, for example because two rules undo each other.
    #[error(
        "the fixes still change the words after {passes} passes; check {} for rules that undo each other",
        rule_names(.rules)
    )]
    NotSettled {
        /// How many times the rules were applied.
        passes: usize,
        /// Indexes of the rules that still made changes in the last pass.
        rules: Vec<usize>,
    },
}

fn rule_names(rules: &[usize]) -> String {
    rules
        .iter()
        .map(|index| format!("fix[{index}]"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reads only the version, so a file from a newer format gets a clear error.
#[derive(Deserialize)]
struct VersionProbe {
    schema: Option<toml::Value>,
}

impl Fixes {
    /// Fixes with these rules.
    pub fn new(rules: Vec<Fix>) -> Self {
        Self {
            schema: FIXES_SCHEMA_VERSION,
            rules,
        }
    }

    /// Parses a `fixes.toml` document.
    ///
    /// This checks the format version and the shape of the document (unknown keys are
    /// rejected). Call [`Fixes::validate`] to check the rules.
    pub fn from_toml_str(text: &str) -> Result<Self, FixesParseError> {
        let probe: VersionProbe = toml::from_str(text)?;
        match probe.schema {
            None => return Err(FixesParseError::MissingSchema),
            Some(toml::Value::Integer(version)) if version == i64::from(FIXES_SCHEMA_VERSION) => {}
            Some(toml::Value::Integer(version)) => {
                return Err(FixesParseError::UnsupportedSchema(version));
            }
            Some(_) => return Err(FixesParseError::SchemaNotInteger),
        }
        Ok(toml::from_str(text)?)
    }

    /// Checks every rule of schema v1 and reports all problems at once.
    pub fn validate(&self) -> Result<(), ValidationErrors> {
        RuleSet::compile(self).map(|_| ())
    }

    /// Format version.
    pub fn schema(&self) -> u32 {
        self.schema
    }

    /// The rules, in file order (`[[fix]]`).
    pub fn rules(&self) -> &[Fix] {
        &self.rules
    }

    /// The JSON Schema of fixes.toml (schema v1), pretty-printed with a trailing
    /// newline. It is exactly the content of the repository's `schema/fixes-v1.json`.
    pub fn json_schema() -> String {
        crate::schema_gen::pretty::<Self>()
    }
}

impl Fix {
    /// A rule that replaces the words `from` with `to`.
    pub fn new(from: impl Into<String>, to: impl Into<String>, case_sensitive: bool) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            case_sensitive,
        }
    }

    /// The words to find.
    pub fn from(&self) -> &str {
        &self.from
    }

    /// What to write instead.
    pub fn to(&self) -> &str {
        &self.to
    }

    /// Whether capitals must match exactly.
    pub fn case_sensitive(&self) -> bool {
        self.case_sensitive
    }
}

/// The words after the fixes, and what changed.
#[derive(Clone, Debug, PartialEq)]
pub struct Fixed {
    transcript: Transcript,
    changes: Vec<FixChange>,
}

impl Fixed {
    /// The transcript with the fixes applied.
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// Takes the fixed transcript.
    pub fn into_transcript(self) -> Transcript {
        self.transcript
    }

    /// Every change, in word order. Empty when the fixes changed nothing.
    pub fn changes(&self) -> &[FixChange] {
        &self.changes
    }
}

/// One change made by the fixes: some input words and what they became.
#[derive(Clone, Debug, PartialEq)]
pub struct FixChange {
    words: Range<usize>,
    start: f64,
    end: f64,
    before: String,
    after: String,
    rules: Vec<usize>,
}

impl FixChange {
    /// Indexes of the input words that were replaced.
    pub fn words(&self) -> Range<usize> {
        self.words.clone()
    }

    /// Start of the changed words, in seconds.
    pub fn start(&self) -> f64 {
        self.start
    }

    /// End of the changed words, in seconds.
    pub fn end(&self) -> f64 {
        self.end
    }

    /// The input words, joined with spaces.
    pub fn before(&self) -> &str {
        &self.before
    }

    /// What they became, joined with spaces.
    pub fn after(&self) -> &str {
        &self.after
    }

    /// Indexes of the rules that made the change, in the order they applied.
    pub fn rules(&self) -> &[usize] {
        &self.rules
    }
}

/// Applies the fixes to a transcript and returns the new words.
///
/// This is a pure function: the inputs are not modified. Rules are applied from the
/// first word to the last, preferring the rule that matches the most words, and then
/// applied again until nothing changes. The result is therefore idempotent: applying
/// the same fixes to it changes nothing. Word timing is kept: replaced words keep
/// their own times, and joined words run from the start of the first to the end of the
/// last. The speech segments, language and model are unchanged.
pub fn apply_fixes(transcript: &Transcript, fixes: &Fixes) -> Result<Fixed, FixError> {
    let rules = RuleSet::compile(fixes).map_err(FixError::Invalid)?;
    let input = transcript.words();
    let mut entries: Vec<Entry> = input
        .iter()
        .enumerate()
        .map(|(i, word)| Entry::from_word(word, i))
        .collect();
    let mut applications = 0;
    // A chain of rules needs one pass per rule, and a rule that joins repeated words
    // ("the the" -> "the") one pass per halving; anything beyond that is a cycle.
    let max_passes = rules.rules.len() + 64;
    let mut settled = false;
    for _ in 0..max_passes {
        let (next, fired) = rules.pass(&entries, &mut applications);
        entries = next;
        if fired.is_empty() {
            settled = true;
            break;
        }
    }
    if !settled {
        // A few more passes show every rule that takes part in the cycle.
        let mut cycling = Vec::new();
        for _ in 0..CYCLE_PASSES {
            let (next, fired) = rules.pass(&entries, &mut applications);
            entries = next;
            cycling.extend(fired);
        }
        cycling.sort_unstable();
        cycling.dedup();
        return Err(FixError::NotSettled {
            passes: max_passes,
            rules: cycling,
        });
    }
    let changes = changes(input, &entries);
    let words = entries
        .into_iter()
        .map(|entry| Word::new(entry.text, entry.start, entry.end, entry.probability))
        .collect();
    Ok(Fixed {
        transcript: transcript.with_words(words),
        changes,
    })
}

/// A rule ready for matching.
struct Rule {
    index: usize,
    /// The words of `from` without their edge punctuation; lowercase unless the rule
    /// is case-sensitive.
    words: Vec<String>,
    /// `to`, with runs of whitespace collapsed to one space.
    to: String,
    case_sensitive: bool,
}

impl Rule {
    fn matches(&self, core: &str, lower: &str, wanted: &str) -> bool {
        if self.case_sensitive {
            core == wanted
        } else {
            lower == wanted
        }
    }

    fn to_words(&self) -> std::str::SplitWhitespace<'_> {
        self.to.split_whitespace()
    }
}

struct RuleSet {
    rules: Vec<Rule>,
    /// Rules by their lowercase first word, longest first.
    by_first_word: HashMap<String, Vec<usize>>,
}

impl RuleSet {
    fn compile(fixes: &Fixes) -> Result<Self, ValidationErrors> {
        let mut report = Report::default();
        if fixes.schema != FIXES_SCHEMA_VERSION {
            report.push(
                "schema",
                format!("must be {FIXES_SCHEMA_VERSION}, got {}", fixes.schema),
            );
        }
        let mut rules = Vec::with_capacity(fixes.rules.len());
        let mut seen: HashMap<String, usize> = HashMap::new();
        for (index, fix) in fixes.rules.iter().enumerate() {
            let field = format!("fix[{index}]");
            let words = word_cores(&fix.from);
            if words.is_empty() {
                report.push(
                    format!("{field}.from"),
                    format!(
                        "must contain a word (letters or digits), got {:?}",
                        fix.from
                    ),
                );
            } else if words.len() > MAX_FROM_WORDS {
                report.push(
                    format!("{field}.from"),
                    format!(
                        "must have at most {MAX_FROM_WORDS} words, got {}",
                        words.len()
                    ),
                );
            } else {
                let key = words
                    .iter()
                    .map(|word| word.to_lowercase())
                    .collect::<Vec<_>>()
                    .join(" ");
                if let Some(first) = seen.get(&key) {
                    report.push(
                        format!("{field}.from"),
                        format!(
                            "must be unique, but {:?} is already fix[{first}].from",
                            fix.from
                        ),
                    );
                } else {
                    seen.insert(key, index);
                }
            }
            let to = fix.to.split_whitespace().collect::<Vec<_>>().join(" ");
            if to.is_empty() {
                report.push(format!("{field}.to"), "must not be empty");
            } else if to.chars().any(char::is_control) {
                report.push(
                    format!("{field}.to"),
                    "must not contain control characters such as tabs",
                );
            }
            let words = words
                .into_iter()
                .map(|word| {
                    if fix.case_sensitive {
                        word.to_owned()
                    } else {
                        word.to_lowercase()
                    }
                })
                .collect();
            rules.push(Rule {
                index,
                words,
                to,
                case_sensitive: fix.case_sensitive,
            });
        }
        report.finish()?;

        let mut by_first_word: HashMap<String, Vec<usize>> = HashMap::new();
        for (position, rule) in rules.iter().enumerate() {
            if let Some(first) = rule.words.first() {
                by_first_word
                    .entry(first.to_lowercase())
                    .or_default()
                    .push(position);
            }
        }
        for candidates in by_first_word.values_mut() {
            candidates.sort_by_key(|&position| {
                let len = rules.get(position).map_or(0, |rule| rule.words.len());
                (std::cmp::Reverse(len), position)
            });
        }
        Ok(Self {
            rules,
            by_first_word,
        })
    }

    /// Applies the rules once, from the first word to the last. Returns the new
    /// entries and the indexes of the rules that changed something.
    fn pass(&self, entries: &[Entry], applications: &mut usize) -> (Vec<Entry>, Vec<usize>) {
        let scanned: Vec<Scanned> = entries
            .iter()
            .map(|entry| Scanned::new(&entry.text))
            .collect();
        let mut out = Vec::with_capacity(entries.len());
        let mut fired = Vec::new();
        let mut position = 0;
        while let (Some(rest), Some(rest_scanned)) =
            (entries.get(position..), scanned.get(position..))
        {
            let Some(first) = rest.first() else {
                break;
            };
            match self.longest_match(rest_scanned) {
                Some((rule, len)) => {
                    let matched = rest.get(..len).unwrap_or_default();
                    let matched_scanned = rest_scanned.get(..len).unwrap_or_default();
                    let replaced = replace(matched, matched_scanned, rule);
                    if is_change(matched, &replaced) {
                        *applications += 1;
                        fired.push(rule.index);
                        out.extend(replaced.into_iter().map(|mut entry| {
                            entry.application = Some(*applications);
                            entry
                        }));
                    } else {
                        out.extend_from_slice(matched);
                    }
                    position += len;
                }
                None => {
                    out.push(first.clone());
                    position += 1;
                }
            }
        }
        (out, fired)
    }

    /// The rule that matches the most words at the start of `rest`, and how many
    /// entries it covers.
    fn longest_match(&self, rest: &[Scanned]) -> Option<(&Rule, usize)> {
        let first = rest.first()?.lower.first()?;
        let candidates = self.by_first_word.get(first)?;
        candidates
            .iter()
            .filter_map(|&position| self.rules.get(position))
            .find_map(|rule| covered_entries(rule, rest).map(|len| (rule, len)))
    }
}

/// How many entries at the start of `rest` the rule covers exactly, if it matches.
/// A rule must cover whole entries: it never matches part of a joined word.
fn covered_entries(rule: &Rule, rest: &[Scanned]) -> Option<usize> {
    let mut wanted = rule.words.iter();
    let mut remaining = rule.words.len();
    for (used, scanned) in rest.iter().enumerate() {
        if scanned.cores.is_empty() || scanned.cores.len() > remaining {
            return None;
        }
        for (core, lower) in scanned.cores.iter().zip(&scanned.lower) {
            if !rule.matches(core, lower, wanted.next()?) {
                return None;
            }
        }
        remaining -= scanned.cores.len();
        if remaining == 0 {
            return Some(used + 1);
        }
    }
    None
}

/// The replacement for the entries a rule matched.
fn replace(matched: &[Entry], scanned: &[Scanned], rule: &Rule) -> Vec<Entry> {
    let one_word_each = scanned.iter().all(|s| s.cores.len() == 1);
    if one_word_each && rule.to_words().count() == matched.len() {
        return matched
            .iter()
            .zip(rule.to_words())
            .map(|(entry, word)| {
                let (prefix, suffix) = affixes(&entry.text);
                let mut rules = entry.rules.clone();
                push_unique(&mut rules, rule.index);
                Entry {
                    text: format!("{prefix}{word}{suffix}"),
                    rules,
                    ..entry.clone()
                }
            })
            .collect();
    }
    let (Some(first), Some(last)) = (matched.first(), matched.last()) else {
        return Vec::new();
    };
    let (prefix, _) = affixes(&first.text);
    let (_, suffix) = affixes(&last.text);
    let mut rules = Vec::new();
    for entry in matched {
        for &index in &entry.rules {
            push_unique(&mut rules, index);
        }
    }
    push_unique(&mut rules, rule.index);
    let probability =
        matched.iter().map(|entry| entry.probability).sum::<f64>() / matched.len() as f64;
    vec![Entry {
        text: format!("{prefix}{}{suffix}", rule.to),
        start: first.start,
        end: last.end,
        probability,
        origin: first.origin.start..last.origin.end,
        rules,
        application: None,
    }]
}

fn is_change(matched: &[Entry], replaced: &[Entry]) -> bool {
    matched.len() != replaced.len()
        || matched
            .iter()
            .zip(replaced)
            .any(|(before, after)| before.text != after.text)
}

fn push_unique(rules: &mut Vec<usize>, index: usize) {
    if !rules.contains(&index) {
        rules.push(index);
    }
}

/// A word while the fixes run.
#[derive(Clone, Debug)]
struct Entry {
    text: String,
    start: f64,
    end: f64,
    probability: f64,
    /// The input words it came from.
    origin: Range<usize>,
    /// Rules that changed it, in order.
    rules: Vec<usize>,
    /// The rule application that last changed it, to group reported changes.
    application: Option<usize>,
}

impl Entry {
    fn from_word(word: &Word, index: usize) -> Self {
        Self {
            text: word.text().to_owned(),
            start: word.start(),
            end: word.end(),
            probability: word.probability(),
            origin: index..index + 1,
            rules: Vec::new(),
            application: None,
        }
    }
}

/// The words of an entry's text, without edge punctuation.
struct Scanned {
    cores: Vec<String>,
    lower: Vec<String>,
}

impl Scanned {
    fn new(text: &str) -> Self {
        let cores: Vec<String> = word_cores(text).into_iter().map(str::to_owned).collect();
        let lower = cores.iter().map(|core| core.to_lowercase()).collect();
        Self { cores, lower }
    }
}

fn is_edge_punctuation(c: char) -> bool {
    !c.is_alphanumeric() && !WORD_SYMBOLS.contains(&c)
}

/// The words of `text` (split at whitespace) without the punctuation at their edges.
/// Words that are only punctuation are left out.
fn word_cores(text: &str) -> Vec<&str> {
    text.split_whitespace()
        .map(|word| word.trim_matches(is_edge_punctuation))
        .filter(|core| !core.is_empty())
        .collect()
}

/// The punctuation before the first word core and after the last one, like `("«", "»,")`
/// for `«Supa base»,`.
fn affixes(text: &str) -> (&str, &str) {
    let is_core = |c: char| !c.is_whitespace() && !is_edge_punctuation(c);
    let first = text.find(is_core);
    let last_end = text
        .char_indices()
        .rev()
        .find(|&(_, c)| is_core(c))
        .map(|(i, c)| i + c.len_utf8());
    match (first, last_end) {
        (Some(first), Some(last_end)) => (
            text.get(..first).unwrap_or_default(),
            text.get(last_end..).unwrap_or_default(),
        ),
        _ => ("", ""),
    }
}

/// Groups the changed output words into reported changes.
fn changes(input: &[Word], output: &[Entry]) -> Vec<FixChange> {
    let mut changes: Vec<(Option<usize>, FixChange)> = Vec::new();
    for entry in output {
        let originals = input.get(entry.origin.clone()).unwrap_or_default();
        let unchanged = match originals {
            [only] => only.text() == entry.text,
            _ => false,
        };
        if unchanged {
            continue;
        }
        if let Some((application, change)) = changes.last_mut()
            && application.is_some()
            && *application == entry.application
            && change.words.end == entry.origin.start
        {
            change.words.end = entry.origin.end;
            change.end = entry.end;
            for original in originals {
                change.before.push(' ');
                change.before.push_str(original.text());
            }
            change.after.push(' ');
            change.after.push_str(&entry.text);
            for &index in &entry.rules {
                push_unique(&mut change.rules, index);
            }
            continue;
        }
        changes.push((
            entry.application,
            FixChange {
                words: entry.origin.clone(),
                start: entry.start,
                end: entry.end,
                before: originals
                    .iter()
                    .map(Word::text)
                    .collect::<Vec<_>>()
                    .join(" "),
                after: entry.text.clone(),
                rules: entry.rules.clone(),
            },
        ));
    }
    changes.into_iter().map(|(_, change)| change).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cores_ignore_edge_punctuation_but_keep_symbols() {
        assert_eq!(
            word_cores("«Supa base», l'homme"),
            ["Supa", "base", "l'homme"]
        );
        assert_eq!(
            word_cores("C++, C#! 50% - ... @dct"),
            ["C++", "C#", "50%", "@dct"]
        );
        assert!(word_cores(" « ! » ").is_empty());
    }

    #[test]
    fn affixes_are_the_punctuation_around_the_words() {
        assert_eq!(affixes("«Supa"), ("«", ""));
        assert_eq!(affixes("base»,"), ("", "»,"));
        assert_eq!(affixes("(Visual Studio)."), ("(", ")."));
        assert_eq!(affixes("plain"), ("", ""));
        assert_eq!(affixes("..."), ("", ""));
    }
}
