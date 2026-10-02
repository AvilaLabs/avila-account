//! The list of Avila Labs tools the launcher shows, with the viewer's fields
//! first. Parsing is lenient (unknown keys and unknown statuses are kept, not
//! refused) so an old build keeps working against a newer manifest.
//! [`Manifest::validate`] is the strict check.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Major schema identifier. A breaking change gets a new identifier.
pub const SCHEMA: &str = "avila-suite-manifest/1";

/// The manifest this build ships with.
pub const DEFAULT_MANIFEST: &str = include_str!("../assets/suite.json");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub schema: String,
    pub dashboard_url: String,
    pub account_url: String,
    pub fields: Vec<Field>,
    pub tools: Vec<Tool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Field {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tool {
    pub id: String,
    pub name: String,
    /// Two letters drawn on the tool's square tile.
    pub monogram: String,
    pub summary: String,
    pub fields: Vec<String>,
    /// `released` or `preview` today; other values are shown as-is.
    pub status: String,
    #[serde(default)]
    pub web: Option<String>,
    #[serde(default)]
    pub desktop: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

impl Manifest {
    /// Lenient parse for clients.
    pub fn parse(text: &str) -> Result<Self, String> {
        let manifest: Manifest =
            serde_json::from_str(text).map_err(|e| format!("suite manifest is not valid: {e}"))?;
        if manifest.schema != SCHEMA {
            return Err(format!(
                "suite manifest schema {:?} is not {SCHEMA:?}",
                manifest.schema
            ));
        }
        Ok(manifest)
    }

    /// The embedded manifest. It is checked by this crate's tests.
    pub fn embedded() -> Self {
        Self::parse(DEFAULT_MANIFEST).expect("embedded suite manifest")
    }

    /// Strict check for a manifest about to be published.
    pub fn validate(&self) -> Result<(), String> {
        let mut field_ids = HashSet::new();
        for field in &self.fields {
            if field.id.is_empty() || !field_ids.insert(field.id.as_str()) {
                return Err(format!("field id {:?} is empty or repeated", field.id));
            }
        }
        let mut tool_ids = HashSet::new();
        for tool in &self.tools {
            if tool.id.is_empty() || !tool_ids.insert(tool.id.as_str()) {
                return Err(format!("tool id {:?} is empty or repeated", tool.id));
            }
            if tool.monogram.chars().count() != 2 {
                return Err(format!("tool {:?} needs a two-letter monogram", tool.id));
            }
            if tool.fields.is_empty() {
                return Err(format!("tool {:?} belongs to no field", tool.id));
            }
            if let Some(field) = tool.fields.iter().find(|f| !field_ids.contains(f.as_str())) {
                return Err(format!("tool {:?} names unknown field {field:?}", tool.id));
            }
            if tool.web.is_none() && tool.desktop.is_none() && tool.source.is_none() {
                return Err(format!("tool {:?} has no way to open it", tool.id));
            }
        }
        Ok(())
    }

    /// Tools in the viewer's fields first, then everything else, each in
    /// manifest order. With no fields chosen, every tool is "yours".
    pub fn arrange<'a>(&'a self, chosen: &[String]) -> (Vec<&'a Tool>, Vec<&'a Tool>) {
        if chosen.is_empty() {
            return (self.tools.iter().collect(), Vec::new());
        }
        self.tools
            .iter()
            .partition(|t| t.fields.iter().any(|f| chosen.contains(f)))
    }

    pub fn field_name<'a>(&'a self, id: &'a str) -> &'a str {
        self.fields
            .iter()
            .find(|f| f.id == id)
            .map_or(id, |f| f.name.as_str())
    }

    pub fn tool(&self, id: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.id == id)
    }
}

impl Tool {
    /// Where the launcher sends someone: the browser build when one exists.
    pub fn open_url(&self) -> Option<&str> {
        self.web
            .as_deref()
            .or(self.desktop.as_deref())
            .or(self.source.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_manifest_is_valid() {
        let m = Manifest::embedded();
        m.validate().unwrap();
        assert!(m.tool("actinv").is_some());
        // A tool not out yet opens its source repository.
        let faris = m.tool("faris").unwrap();
        assert_eq!(faris.status, "coming soon");
        assert_eq!(faris.open_url(), Some("https://github.com/AvilaLabs/FARIS"));
    }

    #[test]
    fn arrange_puts_chosen_fields_first() {
        let m = Manifest::embedded();
        let (mine, more) = m.arrange(&["medical".into()]);
        assert_eq!(
            mine.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["openbnct"]
        );
        assert_eq!(more.len(), m.tools.len() - 1);
        let (all, none) = m.arrange(&[]);
        assert_eq!(all.len(), m.tools.len());
        assert!(none.is_empty());
    }

    #[test]
    fn newer_manifests_still_parse() {
        let mut v: serde_json::Value = serde_json::from_str(DEFAULT_MANIFEST).unwrap();
        v["announcement"] = "new key from a later release".into();
        v["tools"][0]["status"] = "retired".into();
        v["tools"][0]["extra"] = serde_json::json!({"nested": true});
        let m = Manifest::parse(&v.to_string()).unwrap();
        assert_eq!(m.tools[0].status, "retired");
    }

    #[test]
    fn strict_check_refuses_bad_entries() {
        let mut m = Manifest::embedded();
        m.tools[1].id = m.tools[0].id.clone();
        assert!(m.validate().unwrap_err().contains("repeated"));
        let mut m = Manifest::embedded();
        m.tools[0].fields = vec!["astrophysics".into()];
        assert!(m.validate().unwrap_err().contains("unknown field"));
        assert!(Manifest::parse(&DEFAULT_MANIFEST.replace("/1\"", "/2\"")).is_err());
    }
}
