//! Catalog utilities for discovering and managing view schemas.
//!
//! This module provides utilities for:
//! - Discovering current schema versions from the ViewFactory
//! - Comparing partition schema hashes with current versions
//! - Finding outdated partitions across view sets

use super::view_factory::ViewFactory;
use anyhow::Result;

/// Information about a view set's current schema
#[derive(Debug, Clone)]
pub struct ViewSetInfo {
    pub view_set_name: String,
    pub current_schema_hash: Vec<u8>,
    pub schema: String,
    pub has_view_maker: bool,
    pub global_instance_available: bool,
}

/// List all view sets with their current schema information from the ViewFactory.
pub fn list_view_sets(view_factory: &ViewFactory) -> Result<Vec<ViewSetInfo>> {
    let mut schema_infos = Vec::new();
    let mut processed_view_sets = std::collections::HashSet::new();

    for global_view in view_factory.get_global_views() {
        let view_set_name = global_view.get_view_set_name().to_string();

        if processed_view_sets.contains(&view_set_name) {
            continue;
        }
        processed_view_sets.insert(view_set_name.clone());

        let current_schema_hash = global_view.get_file_schema_hash();
        let schema = format!("{:?}", global_view.get_file_schema());

        // Check if this view set has a ViewMaker (supports non-global instances)
        let has_view_maker = view_factory.get_view_sets().contains_key(&view_set_name);

        schema_infos.push(ViewSetInfo {
            view_set_name,
            current_schema_hash,
            schema,
            has_view_maker,
            global_instance_available: true,
        });
    }

    // Then, collect schema info from view sets that have ViewMakers but no global views
    for (view_set_name, view_maker) in view_factory.get_view_sets() {
        // Skip if we already processed this view set from global views
        if processed_view_sets.contains(view_set_name) {
            continue;
        }

        // Get schema information directly from the ViewMaker
        let current_schema_hash = view_maker.get_schema_hash();
        let schema = format!("{:?}", view_maker.get_schema());

        schema_infos.push(ViewSetInfo {
            view_set_name: view_set_name.clone(),
            current_schema_hash,
            schema,
            has_view_maker: true,
            global_instance_available: false,
        });
    }

    Ok(schema_infos)
}
