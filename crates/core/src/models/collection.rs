use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::AuthConfig;
use super::request::{RequestItem, ScriptConfig};
use super::settings::Variable;

/// Node in a collection tree: either a nested `Folder` or a leaf `RequestItem`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "node_type", rename_all = "snake_case")]
pub enum CollectionNode {
    Folder(Folder),
    Request(RequestItem),
}

/// Hierarchical folder within a collection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Folder {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub items: Vec<CollectionNode>,
    pub auth: AuthConfig,
    pub scripts: ScriptConfig,
}

impl Default for Folder {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "New Folder".to_string(),
            description: String::new(),
            items: Vec::new(),
            auth: AuthConfig::Inherit,
            scripts: ScriptConfig::default(),
        }
    }
}

impl Folder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    pub fn find_request(&self, id: Uuid) -> Option<&RequestItem> {
        find_request_in_nodes(&self.items, id)
    }
}

/// Top-level collection of requests, folders, variables, auth, and scripts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Collection {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub items: Vec<CollectionNode>,
    pub variables: Vec<Variable>,
    pub auth: AuthConfig,
    pub scripts: ScriptConfig,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Default for Collection {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: "New Collection".to_string(),
            description: String::new(),
            items: Vec::new(),
            variables: Vec::new(),
            auth: AuthConfig::NoAuth,
            scripts: ScriptConfig::default(),
            created_at: now,
            updated_at: now,
        }
    }
}

impl Collection {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    pub fn find_request(&self, id: Uuid) -> Option<&RequestItem> {
        find_request_in_nodes(&self.items, id)
    }
}

fn find_request_in_nodes(nodes: &[CollectionNode], id: Uuid) -> Option<&RequestItem> {
    for node in nodes {
        match node {
            CollectionNode::Request(req) if req.id == id => return Some(req),
            CollectionNode::Folder(folder) => {
                if let Some(found) = find_request_in_nodes(&folder.items, id) {
                    return Some(found);
                }
            }
            CollectionNode::Request(_) => {}
        }
    }
    None
}
