use super::{ClaudeCodeAdapter, CodexCliAdapter, ExecutionHostAdapter, L0ManualAdapter};
use awr_core::AdapterCapabilityMatrix;
use std::sync::Arc;

/// Registry of usable coding-agent clients plus the L0 manual reporter.
pub struct AdapterRegistry {
    adapters: Vec<Arc<dyn ExecutionHostAdapter>>,
}

impl AdapterRegistry {
    pub fn new(adapters: Vec<Arc<dyn ExecutionHostAdapter>>) -> Self {
        Self { adapters }
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn ExecutionHostAdapter>> {
        self.adapters
            .iter()
            .find(|a| a.adapter_id().as_str() == id)
            .cloned()
    }

    pub fn matrices(&self) -> Vec<AdapterCapabilityMatrix> {
        self.adapters.iter().map(|a| a.matrix().clone()).collect()
    }

    pub fn named_controlled(&self) -> Vec<Arc<dyn ExecutionHostAdapter>> {
        self.adapters
            .iter()
            .filter(|a| {
                matches!(
                    a.matrix().control_mode,
                    awr_core::AdapterControlMode::NamedControlled
                )
            })
            .cloned()
            .collect()
    }

    pub fn all(&self) -> &[Arc<dyn ExecutionHostAdapter>] {
        &self.adapters
    }
}

pub fn built_in_registry() -> AdapterRegistry {
    AdapterRegistry::new(vec![
        Arc::new(CodexCliAdapter::new()),
        Arc::new(ClaudeCodeAdapter::new()),
        Arc::new(L0ManualAdapter::new()),
    ])
}
