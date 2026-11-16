//! Read-only Batocera library server. SQLite payloads and API v1 remain compatible
//! with the previous Python implementation; indexing never writes to the library.
pub mod api;
pub mod catalog;
pub mod config;
pub mod identity;
pub mod models;
pub mod paths;
pub mod playlist;
pub mod progress;
pub mod reload;
pub mod xml;
