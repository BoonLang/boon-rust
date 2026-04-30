use boon_shape::Shape;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SourceOwner {
    Static,
    DynamicFamily { owner_path: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceEntry {
    pub id: usize,
    pub path: String,
    pub shape: Shape,
    pub producer: String,
    pub readers: Vec<String>,
    pub owner: SourceOwner,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceInventory {
    pub entries: Vec<SourceEntry>,
}

impl SourceInventory {
    pub fn get(&self, path: &str) -> Option<&SourceEntry> {
        self.entries.iter().find(|entry| entry.path == path)
    }
}
