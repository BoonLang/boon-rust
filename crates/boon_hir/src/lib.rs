use boon_syntax::ParsedModule;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HirModule {
    pub parsed: ParsedModule,
}

pub fn lower(parsed: ParsedModule) -> HirModule {
    HirModule { parsed }
}
