use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct NodeId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NodeKind {
    Root,
    Panel,
    Text,
    Button,
    TextInput,
    Checkbox,
    Grid,
    Game,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum HostPatch {
    CreateNode {
        id: NodeId,
        kind: NodeKind,
        parent: Option<NodeId>,
        key: Option<String>,
    },
    RemoveNode {
        id: NodeId,
    },
    SetText {
        id: NodeId,
        text: String,
    },
    SetTag {
        id: NodeId,
        tag: String,
    },
    SetSourceBinding {
        id: NodeId,
        source_path: String,
    },
    SetGridCell {
        id: NodeId,
        row: usize,
        col: usize,
        value: String,
    },
    ReplaceFrameText {
        text: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FrameSnapshot {
    pub width: u32,
    pub height: u32,
    pub text: String,
    pub rgba_hash: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FrameInfo {
    pub hash: String,
    pub nonblank: bool,
}
