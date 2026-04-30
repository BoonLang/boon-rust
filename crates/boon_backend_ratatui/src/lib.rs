use anyhow::Result;
use boon_render_ir::{FrameInfo, FrameSnapshot, HostPatch};
use boon_runtime::{BoonApp, SourceBatch};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RatatuiBackend {
    frame_text: String,
    pub width: u16,
    pub height: u16,
}

impl RatatuiBackend {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            frame_text: String::new(),
            width,
            height,
        }
    }

    pub fn load<A: BoonApp>(&mut self, app: &mut A) -> Result<FrameInfo> {
        let turn = app.mount();
        self.apply_patches(&turn.patches)?;
        self.render_frame()
    }

    pub fn dispatch<A: BoonApp>(&mut self, app: &mut A, batch: SourceBatch) -> Result<FrameInfo> {
        for result in app.dispatch_batch(batch)? {
            self.apply_patches(&result.patches)?;
        }
        self.render_frame()
    }

    pub fn apply_patches(&mut self, patches: &[HostPatch]) -> Result<()> {
        for patch in patches {
            if let HostPatch::ReplaceFrameText { text } = patch {
                self.frame_text = text.clone();
            }
        }
        Ok(())
    }

    pub fn render_frame(&self) -> Result<FrameInfo> {
        Ok(FrameInfo {
            hash: stable_hash(&self.frame_text),
            nonblank: !self.frame_text.trim().is_empty(),
        })
    }

    pub fn capture_frame(&self) -> Result<FrameSnapshot> {
        Ok(FrameSnapshot {
            width: self.width as u32,
            height: self.height as u32,
            text: self.frame_text.clone(),
            rgba_hash: None,
        })
    }
}

pub fn stable_hash(text: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
