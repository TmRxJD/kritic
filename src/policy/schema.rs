//! The game's schema definition: state layout, action space and diagnostic thresholds.
//! kritic never interprets a field; it only decodes numbers and applies the game's thresholds.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Numeric element type of a state field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DType {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
    F64,
}

impl DType {
    pub fn size(self) -> usize {
        match self {
            DType::U8 | DType::I8 => 1,
            DType::U16 | DType::I16 => 2,
            DType::U32 | DType::I32 | DType::F32 => 4,
            DType::F64 => 8,
        }
    }

    fn read(self, b: &[u8], big: bool) -> f64 {
        macro_rules! rd {
            ($t:ty) => {{
                let a = b.try_into().expect("sized slice");
                (if big {
                    <$t>::from_be_bytes(a)
                } else {
                    <$t>::from_le_bytes(a)
                }) as f64
            }};
        }
        match self {
            DType::U8 => b[0] as f64,
            DType::I8 => b[0] as i8 as f64,
            DType::U16 => rd!(u16),
            DType::I16 => rd!(i16),
            DType::U32 => rd!(u32),
            DType::I32 => rd!(i32),
            DType::F32 => rd!(f32),
            DType::F64 => rd!(f64),
        }
    }

    fn write(self, v: f64, big: bool, out: &mut Vec<u8>) {
        macro_rules! wr {
            ($t:ty) => {{
                let x = v as $t;
                out.extend_from_slice(&if big {
                    x.to_be_bytes()
                } else {
                    x.to_le_bytes()
                });
            }};
        }
        match self {
            DType::U8 => out.push(v as u8),
            DType::I8 => out.push(v as i8 as u8),
            DType::U16 => wr!(u16),
            DType::I16 => wr!(i16),
            DType::U32 => wr!(u32),
            DType::I32 => wr!(i32),
            DType::F32 => wr!(f32),
            DType::F64 => wr!(f64),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Endian {
    #[default]
    Little,
    Big,
}

fn one() -> usize {
    1
}

/// One named field of the state vector; `count` > 1 makes it a fixed-size array.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct StateField {
    pub name: String,
    pub dtype: DType,
    #[serde(default = "one")]
    pub count: usize,
    /// Expected range, used to normalize diagnostics. Observed range is used when absent.
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
}

/// Packed byte layout of `state_t`: fields in order, no padding.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct StateLayout {
    #[serde(default)]
    pub endian: Endian,
    pub fields: Vec<StateField>,
}

/// One discrete action parameter with `values` choices (0..values-1).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ActionHead {
    pub name: String,
    pub values: u32,
    /// Optional opaque labels, one per value, passed to policies that read text.
    #[serde(default)]
    pub labels: Vec<String>,
}

/// How the action heads are compressed into `action_t`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionPacking {
    /// One byte per head, in order (each head has at most 256 values).
    #[default]
    BytePerHead,
    /// All heads in one little-endian u32: value = a0 + n0*(a1 + n1*(a2 + ...)).
    MixedRadixU32,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ActionLayout {
    pub heads: Vec<ActionHead>,
    #[serde(default)]
    pub packing: ActionPacking,
}

/// A metric that, once past its limit for `for_ticks` and never back, marks the run unrecoverable.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct UnrecoverableRule {
    /// Field reference: `name` or `name[i]`.
    pub field: String,
    #[serde(default)]
    pub below: Option<f64>,
    #[serde(default)]
    pub above: Option<f64>,
    #[serde(default = "one")]
    pub for_ticks: usize,
}

fn d_stuck_ticks() -> usize {
    30
}
fn d_stuck_eps() -> f64 {
    0.001
}
fn d_osc_window() -> usize {
    20
}
fn d_osc_flips() -> usize {
    8
}
fn d_osc_amp() -> f64 {
    0.25
}
fn d_radius() -> f64 {
    0.15
}

/// Reward/progress thresholds the diagnostics apply. Fractions are of each field's range.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct Thresholds {
    /// Field reference of the progress metric (higher is better).
    pub progress_field: String,
    /// A run succeeds once progress reaches this value.
    pub success_at: f64,
    /// Progress counts as made when it rises more than this above its first value.
    #[serde(default)]
    pub progress_epsilon: f64,
    /// Fields watched for stuck states and oscillation (default: all).
    #[serde(default)]
    pub watch_fields: Vec<String>,
    /// A stuck state lasts at least this many ticks with every watched value inside the band.
    #[serde(default = "d_stuck_ticks")]
    pub stuck_ticks: usize,
    /// Band width for a stuck state, as a fraction of each field's range.
    #[serde(default = "d_stuck_eps")]
    pub stuck_epsilon: f64,
    #[serde(default = "d_osc_window")]
    pub oscillation_window: usize,
    /// Direction reversals within the window that count as oscillation.
    #[serde(default = "d_osc_flips")]
    pub oscillation_flips: usize,
    /// Smallest per-tick change, as a fraction of range, that counts toward a reversal.
    #[serde(default = "d_osc_amp")]
    pub oscillation_amplitude: f64,
    #[serde(default)]
    pub unrecoverable: Vec<UnrecoverableRule>,
    /// Families at or below this success rate are vetoed (default 0: only all-fail families).
    #[serde(default)]
    pub min_family_success: f64,
    /// Clustering radius in normalized signature units (RMS distance, 0..1).
    #[serde(default = "d_radius")]
    pub cluster_radius: f64,
}

/// The whole game-supplied definition.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GameSchema {
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// The game's native tick rate; the per-tick policy budget is 1/tick_hz.
    pub tick_hz: f64,
    pub state: StateLayout,
    pub action: ActionLayout,
    pub thresholds: Thresholds,
}

impl GameSchema {
    /// Parse and validate. Every problem is reported, not just the first.
    pub fn parse(text: &str) -> Result<Self, Vec<String>> {
        let s: GameSchema = serde_json::from_str(text).map_err(|e| vec![format!("parse: {e}")])?;
        let errs = s.validate();
        if errs.is_empty() { Ok(s) } else { Err(errs) }
    }

    pub fn validate(&self) -> Vec<String> {
        let mut e = Vec::new();
        if self.name.trim().is_empty() {
            e.push("name is empty".into());
        }
        if !(self.tick_hz.is_finite() && self.tick_hz > 0.0) {
            e.push("tick_hz must be > 0".into());
        }
        if self.state.fields.is_empty() {
            e.push("state.fields is empty".into());
        }
        let mut seen = std::collections::HashSet::new();
        for f in &self.state.fields {
            if !seen.insert(f.name.as_str()) {
                e.push(format!("duplicate state field `{}`", f.name));
            }
            if f.count == 0 {
                e.push(format!("state field `{}` has count 0", f.name));
            }
            if let (Some(a), Some(b)) = (f.min, f.max)
                && a >= b
            {
                e.push(format!("state field `{}` has min >= max", f.name));
            }
        }
        if self.action.heads.is_empty() {
            e.push("action.heads is empty".into());
        }
        let mut product: u64 = 1;
        let mut heads = std::collections::HashSet::new();
        for h in &self.action.heads {
            if !heads.insert(h.name.as_str()) {
                e.push(format!("duplicate action head `{}`", h.name));
            }
            if h.values < 2 {
                e.push(format!("action head `{}` needs at least 2 values", h.name));
            }
            if !h.labels.is_empty() && h.labels.len() != h.values as usize {
                e.push(format!(
                    "action head `{}` has {} labels for {} values",
                    h.name,
                    h.labels.len(),
                    h.values
                ));
            }
            if self.action.packing == ActionPacking::BytePerHead && h.values > 256 {
                e.push(format!(
                    "action head `{}` exceeds 256 values for byte_per_head",
                    h.name
                ));
            }
            product = product.saturating_mul(h.values.max(1) as u64);
        }
        if self.action.packing == ActionPacking::MixedRadixU32 && product > u32::MAX as u64 + 1 {
            e.push("action space does not fit mixed_radix_u32".into());
        }
        let t = &self.thresholds;
        let mut refs = vec![t.progress_field.as_str()];
        refs.extend(t.watch_fields.iter().map(String::as_str));
        refs.extend(t.unrecoverable.iter().map(|r| r.field.as_str()));
        for r in refs {
            if let Err(m) = self.index_of(r) {
                e.push(format!("thresholds: {m}"));
            }
        }
        for r in &t.unrecoverable {
            if r.below.is_none() && r.above.is_none() {
                e.push(format!(
                    "unrecoverable rule on `{}` needs below or above",
                    r.field
                ));
            }
        }
        if t.stuck_ticks < 2 || t.oscillation_window < 2 || t.oscillation_flips < 1 {
            e.push(
                "stuck_ticks and oscillation_window must be >= 2, oscillation_flips >= 1".into(),
            );
        }
        if !(0.0..=1.0).contains(&t.min_family_success) {
            e.push("min_family_success must be within 0..1".into());
        }
        e
    }

    /// Flattened number of state dimensions.
    pub fn dims(&self) -> usize {
        self.state.fields.iter().map(|f| f.count).sum()
    }

    pub fn state_bytes(&self) -> usize {
        self.state
            .fields
            .iter()
            .map(|f| f.count * f.dtype.size())
            .sum()
    }

    pub fn action_bytes(&self) -> usize {
        match self.action.packing {
            ActionPacking::BytePerHead => self.action.heads.len(),
            ActionPacking::MixedRadixU32 => 4,
        }
    }

    /// Per-tick policy budget in microseconds.
    pub fn tick_budget_us(&self) -> f64 {
        1e6 / self.tick_hz
    }

    /// Human label per flattened dimension: `name` or `name[i]`.
    pub fn dim_labels(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.dims());
        for f in &self.state.fields {
            if f.count == 1 {
                out.push(f.name.clone());
            } else {
                out.extend((0..f.count).map(|i| format!("{}[{i}]", f.name)));
            }
        }
        out
    }

    /// Declared (min, max) per flattened dimension.
    pub fn declared_ranges(&self) -> Vec<(Option<f64>, Option<f64>)> {
        self.state
            .fields
            .iter()
            .flat_map(|f| std::iter::repeat_n((f.min, f.max), f.count))
            .collect()
    }

    /// Resolve `name` or `name[i]` to a flattened dimension index.
    pub fn index_of(&self, r: &str) -> Result<usize, String> {
        let (name, idx) = match r.find('[') {
            Some(p) if r.ends_with(']') => {
                let i: usize = r[p + 1..r.len() - 1]
                    .parse()
                    .map_err(|_| format!("bad index in `{r}`"))?;
                (&r[..p], Some(i))
            }
            _ => (r, None),
        };
        let mut base = 0;
        for f in &self.state.fields {
            if f.name == name {
                let i = idx.unwrap_or(0);
                if idx.is_none() && f.count > 1 {
                    return Err(format!("`{name}` is an array; use `{name}[i]`"));
                }
                if i >= f.count {
                    return Err(format!("`{r}` is out of range (count {})", f.count));
                }
                return Ok(base + i);
            }
            base += f.count;
        }
        Err(format!("unknown state field `{name}`"))
    }

    /// Decode a raw `state_t` buffer into a flat numeric vector.
    pub fn decode_state(&self, raw: &[u8]) -> Result<Vec<f64>, String> {
        if raw.len() != self.state_bytes() {
            return Err(format!(
                "state buffer is {} bytes, schema says {}",
                raw.len(),
                self.state_bytes()
            ));
        }
        let big = self.state.endian == Endian::Big;
        let mut out = Vec::with_capacity(self.dims());
        let mut at = 0;
        for f in &self.state.fields {
            let n = f.dtype.size();
            for _ in 0..f.count {
                out.push(f.dtype.read(&raw[at..at + n], big));
                at += n;
            }
        }
        Ok(out)
    }

    /// Encode a flat vector into `state_t` (what a game client does; used by tests and tools).
    pub fn encode_state(&self, v: &[f64]) -> Result<Vec<u8>, String> {
        if v.len() != self.dims() {
            return Err(format!(
                "state has {} values, schema says {}",
                v.len(),
                self.dims()
            ));
        }
        let big = self.state.endian == Endian::Big;
        let mut out = Vec::with_capacity(self.state_bytes());
        let mut i = 0;
        for f in &self.state.fields {
            for _ in 0..f.count {
                f.dtype.write(v[i], big, &mut out);
                i += 1;
            }
        }
        Ok(out)
    }

    /// Compress one value per head into `action_t`.
    pub fn encode_action(&self, a: &[u32]) -> Result<Vec<u8>, String> {
        let heads = &self.action.heads;
        if a.len() != heads.len() {
            return Err(format!(
                "action has {} values, schema has {} heads",
                a.len(),
                heads.len()
            ));
        }
        for (v, h) in a.iter().zip(heads) {
            if *v >= h.values {
                return Err(format!(
                    "action value {v} out of range for head `{}`",
                    h.name
                ));
            }
        }
        Ok(match self.action.packing {
            ActionPacking::BytePerHead => a.iter().map(|v| *v as u8).collect(),
            ActionPacking::MixedRadixU32 => {
                let mut x: u64 = 0;
                for (v, h) in a.iter().zip(heads).rev() {
                    x = x * h.values as u64 + *v as u64;
                }
                (x as u32).to_le_bytes().to_vec()
            }
        })
    }

    pub fn decode_action(&self, raw: &[u8]) -> Result<Vec<u32>, String> {
        let heads = &self.action.heads;
        if raw.len() != self.action_bytes() {
            return Err(format!(
                "action buffer is {} bytes, schema says {}",
                raw.len(),
                self.action_bytes()
            ));
        }
        let out: Vec<u32> = match self.action.packing {
            ActionPacking::BytePerHead => raw.iter().map(|b| *b as u32).collect(),
            ActionPacking::MixedRadixU32 => {
                let mut x = u32::from_le_bytes(raw.try_into().expect("4 bytes")) as u64;
                heads
                    .iter()
                    .map(|h| {
                        let v = x % h.values as u64;
                        x /= h.values as u64;
                        v as u32
                    })
                    .collect()
            }
        };
        for (v, h) in out.iter().zip(heads) {
            if *v >= h.values {
                return Err(format!(
                    "action value {v} out of range for head `{}`",
                    h.name
                ));
            }
        }
        Ok(out)
    }
}

/// The JSON Schema of the schema definition itself, for game authors and validators.
pub fn meta_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(GameSchema)).unwrap_or_default()
}
