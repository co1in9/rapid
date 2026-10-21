use gpui_kit::*;

use crate::engine::{model::Workspace, storage};

/// Owns the persisted workspace. Every mutation is written to disk.
pub struct Store {
    pub ws: Workspace,
}

impl Store {
    pub fn load() -> Self {
        Self {
            ws: storage::load(),
        }
    }

    pub fn update<R>(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Workspace) -> R) -> R {
        let result = f(&mut self.ws);
        if let Err(err) = storage::save(&self.ws) {
            eprintln!("rapid: failed to save workspace: {err:#}");
        }
        cx.notify();
        result
    }
}
