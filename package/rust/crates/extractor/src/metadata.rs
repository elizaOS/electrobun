//! Application metadata embedded in (or shipped alongside) the installer.
//!
//! The CLI writes a small JSON blob describing the app between the metadata and
//! archive markers (Windows/Linux self-extracting binary) or to a sidecar
//! `metadata.json` file (macOS bundle, Windows `.installer/` directory).

use serde::Deserialize;

/// Identity of the app being installed.
///
/// `hash` is the content hash of the compressed bundle. It is present for every
/// real build but kept optional because the embedded-metadata path historically
/// tolerated its absence.
#[derive(Debug, Clone, Deserialize)]
pub struct AppMetadata {
    pub identifier: String,
    pub name: String,
    pub channel: String,
    #[serde(default)]
    pub hash: Option<String>,
}

impl AppMetadata {
    /// Parse the JSON metadata blob. Unknown fields are ignored to stay
    /// forward-compatible with newer CLI versions.
    pub fn parse(bytes: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(bytes)
    }

    /// Build the on-disk bundle name from the display name and channel.
    ///
    /// Matches the CLI's sanitization: strip spaces, turn dots into dashes, and
    /// append `-<channel>` for every channel except `stable`.
    pub fn bundle_base_name(&self) -> String {
        let sanitized: String = self
            .name
            .chars()
            .filter(|c| *c != ' ')
            .map(|c| if c == '.' { '-' } else { c })
            .collect();

        if self.channel == "stable" {
            sanitized
        } else {
            format!("{sanitized}-{}", self.channel)
        }
    }
}
