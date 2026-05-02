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
    ReplaceFrameScene {
        scene: FrameScene,
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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct FrameScene {
    pub title: String,
    pub commands: Vec<DrawCommand>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DrawCommand {
    Rect {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        color: [u8; 4],
    },
    RectOutline {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        color: [u8; 4],
    },
    Text {
        x: u32,
        y: u32,
        scale: u32,
        text: String,
        color: [u8; 4],
    },
}
