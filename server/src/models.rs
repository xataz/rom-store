use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct File {
    pub id: String,
    pub role: String,
    pub destination: String,
    pub size: u64,
    #[schema(pattern = "^[0-9a-f]{64}$")]
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct System {
    pub id: String,
    pub name: String,
    pub game_count: u64,
    #[serde(default)]
    pub image: Option<File>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SystemSummary {
    pub id: String,
    pub name: String,
    pub games: u64,
    pub installable: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct Game {
    pub id: String,
    pub system: String,
    pub path: String,
    pub name: String,
    pub metadata: BTreeMap<String, String>,
    pub installable: bool,
    pub unavailable_reason: Option<String>,
    pub files: Vec<File>,
    #[serde(default)]
    pub revision: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GamePage {
    pub items: Vec<Game>,
    pub total: u64,
    pub offset: u64,
    pub limit: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct Manifest {
    pub version: u32,
    pub game_id: String,
    pub system: String,
    pub path: String,
    pub revision: String,
    pub metadata: BTreeMap<String, String>,
    pub files: Vec<File>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Health {
    pub status: String,
    pub version: String,
    pub manifest_version: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReloadPhase {
    #[default]
    Idle,
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, ToSchema)]
pub struct ReloadStatus {
    pub status: ReloadPhase,
    pub systems: u64,
    pub games: u64,
    pub diagnostics: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct Diagnostic {
    pub system: String,
    pub game: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CatalogIssue {
    pub system: String,
    pub path: String,
    pub name: Option<String>,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct IssuePage {
    pub items: Vec<CatalogIssue>,
    pub total: u64,
    pub offset: u64,
    pub limit: u64,
}

#[derive(Debug, Serialize)]
pub struct IndexReport {
    pub systems: u64,
    pub games: u64,
    pub diagnostics: Vec<Diagnostic>,
    #[serde(skip)]
    pub reused: u64,
    #[serde(skip)]
    pub calculated: u64,
}
