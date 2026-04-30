use boon_shape::Shape;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostSourceLeaf {
    pub relative_path: &'static str,
    pub shape: Shape,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostContract {
    pub function: &'static str,
    pub source_arg: &'static str,
    pub optional_leaves: Vec<HostSourceLeaf>,
}

impl HostContract {
    pub fn accepts(&self, relative_path: &str) -> Option<&Shape> {
        self.optional_leaves
            .iter()
            .find(|leaf| leaf.relative_path == relative_path)
            .map(|leaf| &leaf.shape)
    }
}

pub fn element_contracts() -> Vec<HostContract> {
    vec![
        HostContract {
            function: "Element/button",
            source_arg: "element",
            optional_leaves: vec![
                HostSourceLeaf {
                    relative_path: "event.press",
                    shape: Shape::EmptyRecord,
                },
                HostSourceLeaf {
                    relative_path: "hovered",
                    shape: Shape::tag_false_true(),
                },
                HostSourceLeaf {
                    relative_path: "focused",
                    shape: Shape::tag_false_true(),
                },
            ],
        },
        HostContract {
            function: "Element/text_input",
            source_arg: "element",
            optional_leaves: vec![
                HostSourceLeaf {
                    relative_path: "text",
                    shape: Shape::Text,
                },
                HostSourceLeaf {
                    relative_path: "event.change",
                    shape: Shape::EmptyRecord,
                },
                HostSourceLeaf {
                    relative_path: "event.key_down.key",
                    shape: Shape::key_tags(),
                },
                HostSourceLeaf {
                    relative_path: "event.blur",
                    shape: Shape::EmptyRecord,
                },
                HostSourceLeaf {
                    relative_path: "event.focus",
                    shape: Shape::EmptyRecord,
                },
                HostSourceLeaf {
                    relative_path: "focused",
                    shape: Shape::tag_false_true(),
                },
            ],
        },
        HostContract {
            function: "Element/checkbox",
            source_arg: "element",
            optional_leaves: vec![
                HostSourceLeaf {
                    relative_path: "event.click",
                    shape: Shape::EmptyRecord,
                },
                HostSourceLeaf {
                    relative_path: "checked",
                    shape: Shape::tag_false_true(),
                },
                HostSourceLeaf {
                    relative_path: "hovered",
                    shape: Shape::tag_false_true(),
                },
            ],
        },
        HostContract {
            function: "Element/label",
            source_arg: "element",
            optional_leaves: vec![
                HostSourceLeaf {
                    relative_path: "event.tick",
                    shape: Shape::EmptyRecord,
                },
                HostSourceLeaf {
                    relative_path: "event.frame",
                    shape: Shape::EmptyRecord,
                },
                HostSourceLeaf {
                    relative_path: "event.key_down.key",
                    shape: Shape::key_tags(),
                },
                HostSourceLeaf {
                    relative_path: "event.double_click",
                    shape: Shape::EmptyRecord,
                },
                HostSourceLeaf {
                    relative_path: "hovered",
                    shape: Shape::tag_false_true(),
                },
            ],
        },
        HostContract {
            function: "Element/grid",
            source_arg: "element",
            optional_leaves: vec![HostSourceLeaf {
                relative_path: "event.key_down.key",
                shape: Shape::key_tags(),
            }],
        },
    ]
}

pub fn shape_for_relative_source(relative_path: &str) -> Option<Shape> {
    element_contracts()
        .into_iter()
        .find_map(|contract| contract.accepts(relative_path).cloned())
}
