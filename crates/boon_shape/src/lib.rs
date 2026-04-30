use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Unknown,
    EmptyRecord,
    Record(Vec<(String, Shape)>),
    List(Box<Shape>),
    Text,
    Number,
    TagSet(Vec<String>),
    Function,
    SourceMarker,
    Skip,
    Union(Vec<Shape>),
}

impl Shape {
    pub fn tag_false_true() -> Self {
        Self::TagSet(vec!["False".to_string(), "True".to_string()])
    }

    pub fn key_tags() -> Self {
        Self::TagSet(vec![
            "Enter".to_string(),
            "Escape".to_string(),
            "Backspace".to_string(),
            "Character".to_string(),
            "ArrowUp".to_string(),
            "ArrowDown".to_string(),
            "ArrowLeft".to_string(),
            "ArrowRight".to_string(),
        ])
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::EmptyRecord => "EmptyRecord",
            Self::Record(_) => "Record",
            Self::List(_) => "List",
            Self::Text => "Text",
            Self::Number => "Number",
            Self::TagSet(_) => "TagSet",
            Self::Function => "Function",
            Self::SourceMarker => "SourceMarker",
            Self::Skip => "Skip",
            Self::Union(_) => "Union",
        }
    }
}
