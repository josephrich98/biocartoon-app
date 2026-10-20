//! Synchronous catalog access for the command line and the MCP server (desktop only): the
//! catalog side is [`biocartoon_catalog::query`] (re-exported here); this module adds the
//! editor's template list.

pub use biocartoon_catalog::query::*;
use crate::templates::TemplateStore;
use serde::Serialize;
use std::path::Path;

/// Query terms the way the FTS query sees them (alphanumeric, at least two characters).
fn terms(query: &str) -> Vec<String> {
    biocartoon_catalog::catalog::Worker::query_words(query).into_iter().filter(|t| t.chars().count() >= 2).collect()
}

#[derive(Clone, Debug, Serialize)]
pub struct Template {
    /// `builtin:<n>` for the editor's layouts, otherwise the id in `templates.sqlite`.
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub author: String,
    pub license: String,
    pub created_at: String,
    /// True for the editor's built-in layouts (no file, no thumbnail).
    pub builtin: bool,
    /// True for templates made on this computer.
    pub local: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_url: Option<String>,
}

/// Every template the editor would list: its built-in layouts, then the ones in
/// `<app dir>/templates.sqlite` (the user's own and the published library). `query` filters by
/// words in the title, description or category (all must appear).
pub fn templates(query: &str) -> Vec<Template> {
    let base = asset_base_url();
    let mut out: Vec<Template> = crate::app::BioCartoon::template_names()
        .iter()
        .enumerate()
        .map(|(i, (title, desc))| Template {
            id: format!("builtin:{i}"),
            title: title.to_string(),
            description: desc.to_string(),
            category: "Layout".into(),
            author: String::new(),
            license: String::new(),
            created_at: String::new(),
            builtin: true,
            local: true,
            thumb_url: None,
        })
        .collect();
    let path = app_dir().join("templates.sqlite");
    if path.exists() {
        for r in TemplateStore::open(&path).list() {
            let thumb = match r.thumb_local.as_deref().filter(|p| Path::new(p).exists()) {
                Some(p) => Some(p.to_string()),
                None if !r.meta.thumb_key.is_empty() => Some(format!("{}/{}", base.trim_end_matches('/'), r.meta.thumb_key)),
                None => None,
            };
            out.push(Template {
                id: r.meta.id,
                title: r.meta.title,
                description: r.meta.description,
                category: r.meta.category,
                author: r.meta.author,
                license: r.meta.license,
                created_at: r.meta.created_at,
                builtin: false,
                local: r.json_local.is_some(),
                thumb_url: thumb,
            });
        }
    }
    let terms = terms(query);
    if !terms.is_empty() {
        out.retain(|t| {
            let hay = format!("{} {} {}", t.title, t.description, t.category).to_lowercase();
            terms.iter().all(|w| hay.contains(w.as_str()))
        });
    }
    out
}
