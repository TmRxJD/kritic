//! Decision specs: the only way anything in kritic asks Kev a question.
//!
//! Kev is a typed-decision model. It reads a small state object and answers typed questions.
//! It does well when the state is a few named, defined fields and each question names the
//! fields it reads and defines every answer. It does badly on raw blobs, unitless numbers,
//! compound questions and undefined scales. Measured on Beat Saber maps (2026-09-27):
//!   * a raw stats blob + "is this playable?": random notes scored higher than ranked maps;
//!   * the same facts as defined fields, one condition per question: stacked notes 0.99 vs
//!     0.02, parity in range 0.03 (broken) vs 0.96 (ranked);
//!   * but numbers Kev had to compare itself (0.794 vs a ranked maximum of 0.009) came out
//!     0.73 "in range", and a question combining five fields ranked an all-down map as
//!     acceptable. So code measures and compares; Kev reads verdicts in a fixed vocabulary;
//!     combining fields is code's job.
//!
//! A `DecisionSpec` declares the state (every field named, typed and defined), the questions
//! (each names the fields it reads in backticks; every answer is defined) and, per question,
//! whether it may affect approval. A question may only gate with recorded calibration
//! evidence; everything else is advisory by construction. `lint` enforces all of this, the
//! test suite lints every built-in spec, and plugin manifests are linted on load.

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::kev::{Answer, Question};

/// How a field's value is written into the state Kev reads.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FieldKind {
    /// Prose Kev reads as-is (a request, a file path, a finding's message).
    Text { max_chars: usize },
    /// Source code or a unified diff, one excerpt at most `max_chars` long.
    Code { max_chars: usize },
    /// A value code has already measured and judged, written as one vocabulary term,
    /// optionally followed by ": " and a short detail (units, the raw number).
    Band { vocabulary: Vocabulary },
}

/// A closed set of terms with definitions. Terms are UPPER_SNAKE words so they stand out in
/// the state and can be quoted verbatim in questions and answer definitions.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Vocabulary {
    pub name: String,
    /// (term, definition), in increasing order of concern where that applies.
    pub terms: Vec<(String, String)>,
}

impl Vocabulary {
    pub fn new(name: &str, terms: &[(&str, &str)]) -> Self {
        Vocabulary {
            name: name.into(),
            terms: terms
                .iter()
                .map(|(t, d)| (t.to_string(), d.to_string()))
                .collect(),
        }
    }
    pub fn has(&self, term: &str) -> bool {
        self.terms.iter().any(|(t, _)| t == term)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: String,
    pub kind: FieldKind,
    /// What the field is, in one sentence. Documentation for spec authors and reviewers.
    pub meaning: String,
}

/// One typed question with every possible answer defined.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Ask {
    /// Yes/no with a probability. One condition, and both answers defined.
    Noul {
        instructions: String,
        yes: String,
        no: String,
    },
    /// One of named options, each defined.
    Choice {
        instructions: String,
        options: Vec<(String, String)>,
    },
    /// An ordered scale, lowest first, each rung "label" + definition.
    Score {
        instructions: String,
        rungs: Vec<(String, String)>,
    },
}

impl Ask {
    pub fn instructions(&self) -> &str {
        match self {
            Ask::Noul { instructions, .. }
            | Ask::Choice { instructions, .. }
            | Ask::Score { instructions, .. } => instructions,
        }
    }
    /// All text Kev sees for this question (instructions and answer definitions).
    fn all_text(&self) -> String {
        match self {
            Ask::Noul {
                instructions,
                yes,
                no,
            } => format!("{instructions} {yes} {no}"),
            Ask::Choice {
                instructions,
                options: o,
            }
            | Ask::Score {
                instructions,
                rungs: o,
            } => {
                let defs: Vec<String> = o.iter().map(|(k, d)| format!("{k} {d}")).collect();
                format!("{instructions} {}", defs.join(" "))
            }
        }
    }
}

/// Which number from an answer a gate compares.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
    /// noul: probability of "yes".
    Yes,
    /// choice: probability of the option named in `option`.
    Option,
    /// score: expected rung index (0 = lowest rung).
    Rung,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Cmp {
    Above,
    Below,
}

/// Whether a question may affect approval.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "gate", rename_all = "snake_case", deny_unknown_fields)]
pub enum Gate {
    /// Reported, never decides. `why` says why it doesn't gate (e.g. not yet calibrated).
    Advisory { why: String },
    /// Fails approval when `measure` is `fails_when` `threshold`.
    Rule {
        measure: Measure,
        /// The option whose probability is compared (choice questions only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        option: Option<String>,
        fails_when: Cmp,
        threshold: f64,
        /// Shown to the user when the rule fails.
        reason: String,
        /// The evidence this threshold separates good from bad: date (YYYY-MM-DD), dataset,
        /// and the measured result. Required; a gate without evidence is refused.
        calibration: String,
        /// Measured on good cases: (false-reject rate, threshold that keeps it), from 0.05 down
        /// to 0 (= beyond every good case seen). A change of n units is judged at a per-unit
        /// rate of about 0.05 / n, so the whole change is falsely rejected at most ~5% of the
        /// time instead of 1 - 0.95^n (77% for a 29-hunk change at a flat threshold).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        good_tail: Vec<(f64, f64)>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuestionSpec {
    pub id: String,
    pub ask: Ask,
    pub gate: Gate,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionSpec {
    pub name: String,
    /// What decision this spec supports, in one sentence.
    pub purpose: String,
    pub fields: Vec<Field>,
    pub questions: Vec<QuestionSpec>,
    /// The decision models the `Rule` gates were calibrated on. Under any other model (for example
    /// cloud Jev) every gate is reported but advisory: a threshold measured on one model's
    /// probabilities says nothing about another's.
    #[serde(default = "default_calibrated_for")]
    pub calibrated_for: Vec<String>,
}

fn default_calibrated_for() -> Vec<String> {
    vec!["kev-4b".to_string()]
}

/// Most fields a yes/no question may read, and a choice or score. Measured: a question that
/// had to combine five fields got an all-down map wrong; one or two fields were reliable.
pub const MAX_READS_NOUL: usize = 2;
pub const MAX_READS_OTHER: usize = 3;

fn ident() -> &'static Regex {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| Regex::new(r"^[a-z][a-z0-9_]*$").unwrap())
}
fn term_re() -> &'static Regex {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| Regex::new(r"^[A-Z][A-Z0-9_]*$").unwrap())
}
fn backticks() -> &'static Regex {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| Regex::new(r"`([^`]+)`").unwrap())
}
fn compound() -> &'static Regex {
    // "or" joining conditions (outside backticks). "and" is allowed: "`task` and `change`".
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)\b(or|and/or|either)\b").unwrap())
}
fn dated() -> &'static Regex {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| Regex::new(r"\b\d{4}-\d{2}-\d{2}\b").unwrap())
}

impl DecisionSpec {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }
    pub fn question(&self, id: &str) -> Option<&QuestionSpec> {
        self.questions.iter().find(|q| q.id == id)
    }

    /// Fields a question names in backticks (instructions and answer definitions).
    pub fn reads(ask: &Ask) -> BTreeSet<String> {
        backticks()
            .captures_iter(&ask.all_text())
            .map(|c| c[1].to_string())
            .collect()
    }

    /// Every rule a spec must satisfy. Empty means valid.
    pub fn lint(&self) -> Vec<String> {
        let mut e = Vec::new();
        let at = |what: &str| format!("spec `{}`: {what}", self.name);
        if !ident().is_match(&self.name) {
            e.push(at("name must be snake_case"));
        }
        if self.purpose.trim().is_empty() {
            e.push(at("purpose is empty"));
        }
        if self.fields.is_empty() {
            e.push(at("declares no fields"));
        }
        if self.questions.is_empty() {
            e.push(at("declares no questions"));
        }
        let mut seen = BTreeSet::new();
        for f in &self.fields {
            let at = |what: &str| format!("spec `{}` field `{}`: {what}", self.name, f.name);
            if !ident().is_match(&f.name) {
                e.push(at("name must be snake_case"));
            }
            if !seen.insert(f.name.clone()) {
                e.push(at("declared twice"));
            }
            if f.meaning.trim().len() < 12 {
                e.push(at("meaning must be a real sentence"));
            }
            match &f.kind {
                FieldKind::Text { max_chars } | FieldKind::Code { max_chars }
                    if *max_chars == 0 =>
                {
                    e.push(at("max_chars is 0"))
                }
                FieldKind::Band { vocabulary } => {
                    if vocabulary.terms.len() < 2 {
                        e.push(at("vocabulary needs at least two terms"));
                    }
                    let mut terms = BTreeSet::new();
                    for (t, d) in &vocabulary.terms {
                        if !term_re().is_match(t) {
                            e.push(at(&format!("term `{t}` must be UPPER_SNAKE")));
                        }
                        if !terms.insert(t) {
                            e.push(at(&format!("term `{t}` declared twice")));
                        }
                        if d.trim().len() < 8 {
                            e.push(at(&format!("term `{t}` has no real definition")));
                        }
                    }
                }
                _ => {}
            }
        }
        let mut ids = BTreeSet::new();
        let mut read_anywhere = BTreeSet::new();
        for q in &self.questions {
            let at = |what: &str| format!("spec `{}` question `{}`: {what}", self.name, q.id);
            if !ident().is_match(&q.id) {
                e.push(at("id must be snake_case"));
            }
            if !ids.insert(q.id.clone()) {
                e.push(at("declared twice"));
            }
            let ins = q.ask.instructions();
            if ins.matches('?').count() != 1 || !ins.trim_end().ends_with('?') {
                e.push(at(
                    "instructions must be exactly one question ending in '?'",
                ));
            }
            let reads = Self::reads(&q.ask);
            if reads.is_empty() {
                e.push(at(
                    "names no state field (write fields in backticks, e.g. `change`)",
                ));
            }
            for r in &reads {
                if self.field(r).is_none() {
                    e.push(at(&format!("names `{r}`, which is not a declared field")));
                }
            }
            read_anywhere.extend(reads.iter().cloned());
            let unquoted = backticks().replace_all(ins, "");
            match &q.ask {
                Ask::Noul { yes, no, .. } => {
                    if compound().is_match(&unquoted) {
                        e.push(at("yes/no instructions must hold one condition (no 'or'); split it into two questions"));
                    }
                    if yes.trim().len() < 8 || no.trim().len() < 8 {
                        e.push(at("both `yes` and `no` must be defined"));
                    }
                    if reads.len() > MAX_READS_NOUL {
                        e.push(at(&format!("reads {} fields; a yes/no question may read at most {MAX_READS_NOUL} (combine in code)", reads.len())));
                    }
                }
                Ask::Choice { options, .. } | Ask::Score { rungs: options, .. } => {
                    if options.len() < 2 {
                        e.push(at("needs at least two options/rungs"));
                    }
                    let mut keys = BTreeSet::new();
                    for (k, d) in options {
                        if !ident().is_match(k) {
                            e.push(at(&format!("option/rung `{k}` must be snake_case")));
                        }
                        if !keys.insert(k) {
                            e.push(at(&format!("option/rung `{k}` declared twice")));
                        }
                        if d.trim().len() < 8 {
                            e.push(at(&format!("option/rung `{k}` has no real definition")));
                        }
                    }
                    if reads.len() > MAX_READS_OTHER {
                        e.push(at(&format!(
                            "reads {} fields; at most {MAX_READS_OTHER} (combine in code)",
                            reads.len()
                        )));
                    }
                }
            }
            // A question over a band field must speak that field's vocabulary.
            for r in &reads {
                if let Some(Field {
                    kind: FieldKind::Band { vocabulary },
                    ..
                }) = self.field(r)
                    && !vocabulary
                        .terms
                        .iter()
                        .any(|(t, _)| q.ask.all_text().contains(t.as_str()))
                {
                    e.push(at(&format!(
                        "reads band `{r}` but uses none of its terms ({})",
                        vocabulary.name
                    )));
                }
            }
            if let Gate::Rule {
                measure,
                option,
                threshold,
                reason,
                calibration,
                good_tail,
                fails_when,
            } = &q.gate
            {
                if !good_tail.is_empty() {
                    let rates: Vec<f64> = good_tail.iter().map(|(r, _)| *r).collect();
                    if rates.first() != Some(&0.05)
                        || rates.last() != Some(&0.0)
                        || rates.windows(2).any(|w| w[1] >= w[0])
                    {
                        e.push(at(
                            "good_tail rates must run from 0.05 down to 0, strictly decreasing",
                        ));
                    }
                    if (good_tail[0].1 - threshold).abs() > 1e-9 {
                        e.push(at("good_tail at 0.05 must equal the gate's threshold"));
                    }
                    let monotone = good_tail.windows(2).all(|w| match fails_when {
                        Cmp::Above => w[1].1 >= w[0].1,
                        Cmp::Below => w[1].1 <= w[0].1,
                    });
                    if !monotone {
                        e.push(at(
                            "good_tail thresholds must get stricter as the rate falls",
                        ));
                    }
                }
                if !dated().is_match(calibration) || calibration.trim().len() < 40 {
                    e.push(at("a gating rule needs calibration evidence: a date (YYYY-MM-DD), the dataset and the measured result"));
                }
                if reason.trim().is_empty() {
                    e.push(at("a gating rule needs a reason"));
                }
                match (measure, &q.ask) {
                    (Measure::Yes, Ask::Noul { .. }) if option.is_none() && (0.0..=1.0).contains(threshold) => {}
                    (Measure::Option, Ask::Choice { options, .. })
                        if option.as_ref().is_some_and(|o| options.iter().any(|(k, _)| k == o)) && (0.0..=1.0).contains(threshold) => {}
                    (Measure::Rung, Ask::Score { rungs, .. }) if option.is_none() && *threshold >= 0.0 && *threshold <= (rungs.len() - 1) as f64 => {}
                    _ => e.push(at("gate measure does not fit the question type, names an unknown option, or the threshold is out of range")),
                }
            }
            if let Gate::Advisory { why } = &q.gate
                && why.trim().len() < 12
            {
                e.push(at("an advisory question must say why it does not gate"));
            }
        }
        for f in &self.fields {
            if !read_anywhere.contains(&f.name) {
                e.push(format!("spec `{}` field `{}`: no question reads it (drop it; Kev should see only what it is asked about)", self.name, f.name));
            }
        }
        e
    }

    /// Build the state for one call. Every declared field, nothing else; text within its
    /// limit (callers chunk, this never truncates); band values must start with a term.
    pub fn state(&self, values: &[(&str, String)]) -> Result<Value, String> {
        let mut out = Map::new();
        for (name, v) in values {
            let f = self
                .field(name)
                .ok_or_else(|| format!("spec `{}`: `{name}` is not a declared field", self.name))?;
            match &f.kind {
                FieldKind::Text { max_chars } | FieldKind::Code { max_chars } => {
                    if v.chars().count() > *max_chars {
                        return Err(format!(
                            "spec `{}`: `{name}` is {} chars > {max_chars}; chunk it before asking",
                            self.name,
                            v.chars().count()
                        ));
                    }
                    if v.trim().is_empty() {
                        return Err(format!("spec `{}`: `{name}` is empty", self.name));
                    }
                }
                FieldKind::Band { vocabulary } => {
                    let term = v.split(':').next().unwrap_or("").trim();
                    if !vocabulary.has(term) {
                        return Err(format!(
                            "spec `{}`: `{name}` = {v:?} does not start with a {} term",
                            self.name, vocabulary.name
                        ));
                    }
                }
            }
            if out
                .insert(name.to_string(), Value::String(v.clone()))
                .is_some()
            {
                return Err(format!("spec `{}`: `{name}` given twice", self.name));
            }
        }
        for f in &self.fields {
            if !out.contains_key(&f.name) {
                return Err(format!("spec `{}`: `{}` is missing", self.name, f.name));
            }
        }
        Ok(Value::Object(out))
    }

    /// The questions in laya's wire format, every answer defined.
    pub fn wire(&self) -> BTreeMap<String, Question> {
        self.questions
            .iter()
            .map(|q| (q.id.clone(), wire_question(&q.ask)))
            .collect()
    }

    /// Only the named questions (for calls that ask a subset).
    pub fn wire_subset(&self, ids: &[&str]) -> BTreeMap<String, Question> {
        self.questions
            .iter()
            .filter(|q| ids.contains(&q.id.as_str()))
            .map(|q| (q.id.clone(), wire_question(&q.ask)))
            .collect()
    }

    /// The failure reason if question `id`'s gate fails for `answer`; None when it passes or
    /// is advisory.
    pub fn check(&self, id: &str, answer: &Answer) -> Option<String> {
        if !self.gates_apply() {
            return None;
        }
        let q = self.question(id)?;
        let Gate::Rule {
            measure,
            option,
            fails_when,
            threshold,
            reason,
            ..
        } = &q.gate
        else {
            return None;
        };
        let v = measured(*measure, option.as_deref(), answer)?;
        let fails = match fails_when {
            Cmp::Above => v > *threshold,
            Cmp::Below => v < *threshold,
        };
        fails.then(|| {
            format!(
                "{reason} ({id} {v:.2} {} {threshold:.2})",
                if *fails_when == Cmp::Above { ">" } else { "<" }
            )
        })
    }

    /// `check` for one value (a single unit, or a value that is not a max over units).
    pub fn check_value(&self, id: &str, v: f64) -> Option<String> {
        self.check_units(id, v, 1)
    }

    /// The threshold for a change judged as the worst of `units` units: the good-case tail entry
    /// whose rate is at most 0.05 / units (the strictest one when the data runs out).
    pub fn threshold_for(&self, id: &str, units: usize) -> Option<f64> {
        let Gate::Rule {
            threshold,
            good_tail,
            ..
        } = &self.question(id)?.gate
        else {
            return None;
        };
        if good_tail.is_empty() || units <= 1 {
            return Some(*threshold);
        }
        let target = 0.05 / units as f64;
        Some(
            good_tail
                .iter()
                .find(|(rate, _)| *rate <= target + 1e-12)
                .or(good_tail.last())
                .map_or(*threshold, |(_, t)| *t),
        )
    }

    /// `check` for the worst value over `units` units, at the family-wise threshold.
    /// Whether this spec's gates may decide under the active model. With no model known yet
    /// (offline tests, fakes) the calibrated default applies.
    pub fn gates_apply(&self) -> bool {
        !self.calibrated_for.is_empty()
    }

    pub fn gates_apply_for(&self, model: &str) -> bool {
        self.calibrated_for.iter().any(|m| m == model) && !model.starts_with("cloud:")
    }

    /// Clone a spec with gates made advisory for this exact decision's provenance.
    pub fn for_model(&self, model: &str) -> Self {
        let mut spec = self.clone();
        if !self.gates_apply_for(model) {
            spec.calibrated_for.clear();
        }
        spec
    }

    pub fn uncalibrated_note(&self) -> Option<String> {
        (!self.gates_apply()).then(|| format!("{}: model judgments are advisory; no calibration for this provider/model/quantization", self.name))
    }

    pub fn check_units(&self, id: &str, v: f64, units: usize) -> Option<String> {
        if !self.gates_apply() {
            return None;
        }
        let q = self.question(id)?;
        let Gate::Rule {
            fails_when, reason, ..
        } = &q.gate
        else {
            return None;
        };
        let t = self.threshold_for(id, units)?;
        let fails = match fails_when {
            Cmp::Above => v > t,
            Cmp::Below => v < t,
        };
        let over = if units > 1 {
            format!(", threshold for {units} units")
        } else {
            String::new()
        };
        fails.then(|| {
            format!(
                "{reason} ({id} {v:.2} {} {t:.2}{over})",
                if *fails_when == Cmp::Above { ">" } else { "<" }
            )
        })
    }

    pub fn gates(&self, id: &str) -> bool {
        matches!(self.question(id).map(|q| &q.gate), Some(Gate::Rule { .. }))
    }

    /// Machine-readable documentation of this spec (for `status` and reviewers).
    pub fn describe(&self) -> Value {
        json!(self)
    }
}

fn measured(m: Measure, option: Option<&str>, a: &Answer) -> Option<f64> {
    match m {
        Measure::Yes | Measure::Rung => Some(a.value),
        Measure::Option => a
            .probabilities
            .as_ref()
            .and_then(|p| p.get(option?))
            .copied(),
    }
}

fn wire_question(ask: &Ask) -> Question {
    match ask {
        Ask::Noul {
            instructions,
            yes,
            no,
        } => Question {
            kind: "noul",
            instructions: instructions.clone(),
            criteria: Some(json!({"true": yes, "false": no})),
        },
        Ask::Choice {
            instructions,
            options,
        } => Question {
            kind: "choice",
            instructions: instructions.clone(),
            criteria: Some(Value::Object(
                options
                    .iter()
                    .map(|(k, d)| (k.clone(), Value::String(d.clone())))
                    .collect(),
            )),
        },
        Ask::Score {
            instructions,
            rungs,
        } => Question {
            kind: "score",
            instructions: instructions.clone(),
            criteria: Some(Value::Array(
                rungs
                    .iter()
                    .map(|(k, d)| Value::String(format!("{k}: {d}")))
                    .collect(),
            )),
        },
    }
}

// ------------------------------------------------------------------ loading

/// Parse and lint a spec from JSON. Errors list every schema and lint problem at once, so a
/// spec author (human or agent) fixes them in one pass instead of one per compile.
pub fn load(json_text: &str) -> Result<DecisionSpec, Vec<String>> {
    let spec: DecisionSpec =
        serde_json::from_str(json_text).map_err(|e| vec![format!("schema: {e}")])?;
    let errors = spec.lint();
    if errors.is_empty() {
        Ok(spec)
    } else {
        Err(errors)
    }
}

/// The JSON Schema every spec file (and plugin `[kev]` block) must satisfy.
pub fn json_schema() -> Value {
    serde_json::to_value(schemars::schema_for!(DecisionSpec)).unwrap_or(Value::Null)
}

macro_rules! builtin_specs {
    ($($name:literal),* $(,)?) => {
        const BUILTIN: &[(&str, &str)] = &[$(($name, include_str!(concat!("../specs/", $name, ".json")))),*];
    };
}
builtin_specs!(
    "code_chunk",
    "code_change",
    "completeness",
    "completeness_digest",
    "map_slice",
    "security_triage",
    "function_maintainability",
    "resource_fit",
    "route",
    "route_codex",
    "context_relevance",
    "ux_novice_step",
    "file_relevance",
    "task_requirements",
);

/// Every built-in spec, parsed and linted. A bad spec file is a build-time test failure and a
/// startup error, never a silent default.
pub fn builtin() -> &'static BTreeMap<String, DecisionSpec> {
    static S: std::sync::OnceLock<BTreeMap<String, DecisionSpec>> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        BUILTIN
            .iter()
            .map(|(name, text)| {
                let spec = load(text).unwrap_or_else(|e| {
                    panic!(
                        "built-in spec {name} is invalid:
  {}",
                        e.join(
                            "
  "
                        )
                    )
                });
                assert_eq!(
                    &spec.name, name,
                    "spec file {name}.json declares name {}",
                    spec.name
                );
                (spec.name.clone(), spec)
            })
            .collect()
    })
}

/// A built-in spec by name.
pub fn get(name: &str) -> &'static DecisionSpec {
    builtin()
        .get(name)
        .unwrap_or_else(|| panic!("no built-in spec {name}"))
}

/// Lint every built-in spec without panicking (for tests and `status`).
pub fn lint_builtin() -> Vec<String> {
    BUILTIN
        .iter()
        .flat_map(|(name, text)| match load(text) {
            Ok(s) if &s.name == name => vec![],
            Ok(s) => vec![format!("{name}.json declares name {}", s.name)],
            Err(e) => e.into_iter().map(|x| format!("{name}.json: {x}")).collect(),
        })
        .collect()
}

// ------------------------------------------------------------------ builders

pub fn text(name: &str, max_chars: usize, meaning: &str) -> Field {
    Field {
        name: name.into(),
        kind: FieldKind::Text { max_chars },
        meaning: meaning.into(),
    }
}
pub fn code(name: &str, max_chars: usize, meaning: &str) -> Field {
    Field {
        name: name.into(),
        kind: FieldKind::Code { max_chars },
        meaning: meaning.into(),
    }
}
pub fn band(name: &str, vocabulary: &Vocabulary, meaning: &str) -> Field {
    Field {
        name: name.into(),
        kind: FieldKind::Band {
            vocabulary: vocabulary.clone(),
        },
        meaning: meaning.into(),
    }
}
pub fn noul(id: &str, instructions: &str, yes: &str, no: &str, gate: Gate) -> QuestionSpec {
    QuestionSpec {
        id: id.into(),
        ask: Ask::Noul {
            instructions: instructions.into(),
            yes: yes.into(),
            no: no.into(),
        },
        gate,
    }
}
pub fn choice(id: &str, instructions: &str, options: &[(&str, &str)], gate: Gate) -> QuestionSpec {
    QuestionSpec {
        id: id.into(),
        ask: Ask::Choice {
            instructions: instructions.into(),
            options: options
                .iter()
                .map(|(k, d)| (k.to_string(), d.to_string()))
                .collect(),
        },
        gate,
    }
}
pub fn score(id: &str, instructions: &str, rungs: &[(&str, &str)], gate: Gate) -> QuestionSpec {
    QuestionSpec {
        id: id.into(),
        ask: Ask::Score {
            instructions: instructions.into(),
            rungs: rungs
                .iter()
                .map(|(k, d)| (k.to_string(), d.to_string()))
                .collect(),
        },
        gate,
    }
}
pub fn advisory(why: &str) -> Gate {
    Gate::Advisory { why: why.into() }
}
pub fn yes_above(threshold: f64, reason: &str, calibration: &str) -> Gate {
    Gate::Rule {
        measure: Measure::Yes,
        option: None,
        fails_when: Cmp::Above,
        threshold,
        reason: reason.into(),
        calibration: calibration.into(),
        good_tail: vec![],
    }
}
pub fn yes_below(threshold: f64, reason: &str, calibration: &str) -> Gate {
    Gate::Rule {
        measure: Measure::Yes,
        option: None,
        fails_when: Cmp::Below,
        threshold,
        reason: reason.into(),
        calibration: calibration.into(),
        good_tail: vec![],
    }
}
