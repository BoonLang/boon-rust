use anyhow::Result;
use boon_render_ir::HostPatch;
pub use boon_source::SourceInventory;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

mod compiled_app;

pub use compiled_app::CompiledApp;
pub type ExampleApp = CompiledApp;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SourceValue {
    EmptyRecord,
    Text(String),
    Number(i64),
    Tag(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceEmission {
    pub path: String,
    pub value: SourceValue,
    pub owner_id: Option<String>,
    pub owner_generation: Option<u32>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceBatch {
    pub state_updates: Vec<SourceEmission>,
    pub events: Vec<SourceEmission>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct TurnId(pub u64);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TurnMetrics {
    pub turn_ms: f64,
    pub patch_count: usize,
    pub events_processed: usize,
    pub dynamic_rows_touched: usize,
    pub dynamic_structure_rebuilds: usize,
    pub source_rebindings: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct StateDelta {
    pub changed_paths: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TurnResult {
    pub turn_id: TurnId,
    pub patches: Vec<HostPatch>,
    pub state_delta: StateDelta,
    pub metrics: TurnMetrics,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppSnapshot {
    pub values: BTreeMap<String, Value>,
    pub frame_text: String,
}

pub trait BoonApp {
    fn mount(&mut self) -> TurnResult;
    fn dispatch_batch(&mut self, batch: SourceBatch) -> Result<Vec<TurnResult>>;
    fn advance_time(&mut self, _delta: Duration) -> TurnResult {
        TurnResult::default()
    }
    fn snapshot(&self) -> AppSnapshot;
    fn source_inventory(&self) -> SourceInventory;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClockTime {
    pub millis: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeClock {
    pub millis: u64,
}

impl RuntimeClock {
    pub fn advance(&mut self, delta: Duration) {
        self.millis += delta.as_millis() as u64;
    }
}
