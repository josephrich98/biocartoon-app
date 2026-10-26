//! Your library's own icons in the web app, and the items a signed-in account keeps online.
//!
//! The browser's icon catalog lives in memory, so the icons added to Your library are also listed
//! in `library.json` (their images are already in [`crate::storage`]) and put back into each
//! catalog loaded. The desktop app keeps them in its catalog file instead and writes no list.
//!
//! Online (see [`crate::account`]), Your library is a set of JSON items: `asset.<slug>` for each
//! own icon (its row, PNG and SVG), `template.<slug>` for each saved template (its row, file and
//! picture) and `favorites`, each with the time it last changed here.

use crate::catalog::{AssetLayer, AssetRow};
use crate::templates::{TemplateMeta, TemplateRow};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Only the browser needs the list: the desktop catalog is a file that keeps its rows.
const KEEP: bool = cfg!(target_arch = "wasm32");

pub const FAVORITES: &str = "favorites";

/// One of the user's own icons, as [`crate::catalog::Worker::insert_user`] stores it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Asset {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub tags: Vec<String>,
    pub license: String,
    pub author: String,
    pub credit: String,
    pub png_key: String,
    pub png_local: Option<String>,
    pub png_width: u32,
    pub png_height: u32,
    pub svg_key: Option<String>,
    pub layers: Vec<AssetLayer>,
    pub generative_ai_used: Option<bool>,
    /// When it was added here (or the copy online was), Unix milliseconds.
    pub modified: i64,
}

impl Asset {
    pub fn from_row(r: &AssetRow, modified: i64) -> Self {
        Self {
            id: r.id.clone(),
            title: r.title.clone(),
            description: r.description.clone(),
            category: r.category.clone(),
            tags: r.tags.clone(),
            license: r.license.clone(),
            author: r.author.clone(),
            credit: r.credit.clone(),
            png_key: r.png_key.clone(),
            png_local: r.png_local.clone(),
            png_width: r.png_width,
            png_height: r.png_height,
            svg_key: r.svg_key.clone(),
            layers: r.layers.clone(),
            generative_ai_used: r.generative_ai_used,
            modified,
        }
    }

    pub fn to_row(&self) -> AssetRow {
        AssetRow {
            id: self.id.clone(),
            source: "user".into(),
            source_name: "User-generated".into(),
            title: self.title.clone(),
            variant: String::new(),
            category: self.category.clone(),
            description: self.description.clone(),
            tags: self.tags.clone(),
            license: self.license.clone(),
            credit: self.credit.clone(),
            author: self.author.clone(),
            png_key: self.png_key.clone(),
            png_local: self.png_local.clone(),
            png_width: self.png_width,
            png_height: self.png_height,
            svg_key: self.svg_key.clone(),
            stars: 0,
            starred: false,
            taxa: Vec::new(),
            layers: self.layers.clone(),
            generative_ai_used: self.generative_ai_used,
            file_bytes: 0,
        }
    }
}

/// What `library.json` keeps.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub assets: Vec<Asset>,
    /// The favorites as last seen, to notice when they change...
    pub favorites: Vec<String>,
    /// ...and when they last did (0: never, in this browser).
    pub favorites_modified: i64,
}

pub struct Library {
    path: PathBuf,
    pub m: Manifest,
}

impl Library {
    pub fn open(path: PathBuf) -> Self {
        let m = if KEEP { crate::storage::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default() } else { Manifest::default() };
        Self { path, m }
    }

    fn save(&self) {
        if !KEEP {
            return;
        }
        let r = serde_json::to_vec(&self.m).map_err(|e| e.to_string()).and_then(|b| crate::storage::write(&self.path, &b).map_err(|e| e.to_string()));
        if let Err(e) = r {
            log::error!("library: {e}");
        }
    }

    pub fn add(&mut self, row: &AssetRow, modified: i64) {
        self.m.assets.retain(|a| a.id != row.id);
        self.m.assets.push(Asset::from_row(row, modified));
        self.save();
    }

    pub fn remove(&mut self, id: &str) {
        let before = self.m.assets.len();
        self.m.assets.retain(|a| a.id != id);
        if self.m.assets.len() != before {
            self.save();
        }
    }

    /// Note the favorites as they are now; true when they changed since last noted.
    pub fn note_favorites(&mut self, favorites: &[String], now: i64) -> bool {
        if self.m.favorites == favorites {
            return false;
        }
        self.m.favorites = favorites.to_vec();
        self.m.favorites_modified = now;
        self.save();
        true
    }
}

/// The online id of an item: `kind.` and its id with anything but `[a-z0-9_-]` made `_` (the
/// item itself carries the real id).
pub fn item_id(kind: &str, id: &str) -> String {
    let id = id.strip_prefix("user/").unwrap_or(id);
    let safe: String = id.chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' { c } else { '_' }).take(160).collect();
    // The Worker wants an id that starts with a letter or digit.
    let safe = if safe.starts_with(['_', '-']) { format!("x{}", &safe[..safe.len().min(159)]) } else { safe };
    format!("{kind}.{safe}")
}

/// When a saved template was saved: its `created_at`, which a save renews.
pub fn template_modified(meta: &TemplateMeta) -> i64 {
    chrono::DateTime::parse_from_rfc3339(&meta.created_at).map(|t| t.timestamp_millis()).unwrap_or(1).max(1)
}

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// An item as it is kept online.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Item {
    Asset {
        asset: Asset,
        /// Base64.
        png: String,
        svg: Option<String>,
    },
    Template {
        meta: TemplateMeta,
        json_key: String,
        /// The folder under `templates/` its files are kept in here.
        dir: String,
        /// The template file's own text.
        file: String,
        /// Base64 PNG.
        thumb: String,
    },
    Favorites {
        favorites: Vec<String>,
    },
}

fn file_name(p: &str) -> Option<String> {
    Path::new(p).file_name().map(|n| n.to_string_lossy().to_string())
}

impl Item {
    pub fn asset(a: &Asset) -> Result<Self, String> {
        let png = a.png_local.as_deref().ok_or("no image")?;
        let png = crate::storage::read(Path::new(png)).map_err(|e| e.to_string())?;
        let svg = a.svg_key.as_deref().and_then(|p| crate::storage::read_to_string(Path::new(p)).ok());
        Ok(Self::Asset { asset: a.clone(), png: B64.encode(png), svg })
    }

    pub fn template(row: &TemplateRow) -> Result<Self, String> {
        let json = PathBuf::from(row.json_local.as_deref().ok_or("not saved here")?);
        let file = crate::storage::read_to_string(&json).map_err(|e| e.to_string())?;
        let thumb = row.thumb_local.as_deref().and_then(|p| crate::storage::read(Path::new(p)).ok()).unwrap_or_default();
        let dir = json.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| row.meta.id.clone());
        Ok(Self::Template { meta: row.meta.clone(), json_key: row.json_key.clone(), dir, file, thumb: B64.encode(thumb) })
    }

    /// Write a downloaded icon's files under `app_dir`; its row, pointing at them.
    pub fn restore_asset(app_dir: &Path, mut asset: Asset, png: &str, svg: Option<String>, modified: i64) -> Result<Asset, String> {
        let dir = app_dir.join("user_generated");
        let png_name = asset.png_local.as_deref().and_then(file_name).filter(|n| n.ends_with(".png")).ok_or("no image")?;
        let png = B64.decode(png).map_err(|e| e.to_string())?;
        let png_path = dir.join(safe_name(&png_name));
        crate::storage::write(&png_path, &png).map_err(|e| e.to_string())?;
        asset.png_local = Some(png_path.display().to_string());
        asset.svg_key = match (svg, asset.svg_key.as_deref().and_then(file_name)) {
            (Some(svg), Some(name)) if name.ends_with(".svg") => {
                let p = dir.join(safe_name(&name));
                crate::storage::write(&p, svg.as_bytes()).map_err(|e| e.to_string())?;
                Some(p.display().to_string())
            }
            _ => None,
        };
        asset.modified = modified;
        Ok(asset)
    }

    /// Write a downloaded template's files under `app_dir`; its row, pointing at them.
    pub fn restore_template(app_dir: &Path, meta: TemplateMeta, json_key: String, dir: &str, file: &str, thumb: &str) -> Result<TemplateRow, String> {
        crate::templates::parse(file.as_bytes())?;
        let dir = app_dir.join("templates").join(safe_name(dir));
        let (json, thumb_path) = (dir.join("template.json"), dir.join("thumb.png"));
        crate::storage::write(&json, file.as_bytes()).map_err(|e| e.to_string())?;
        let thumb = B64.decode(thumb).map_err(|e| e.to_string())?;
        crate::storage::write(&thumb_path, &thumb).map_err(|e| e.to_string())?;
        Ok(TemplateRow { meta, json_key, json_local: Some(json.display().to_string()), thumb_local: Some(thumb_path.display().to_string()) })
    }
}

/// A file or folder name from elsewhere, kept to one plain path component.
fn safe_name(s: &str) -> String {
    let s: String = s.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).collect();
    if s.is_empty() || s == "." || s == ".." { "_".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_ids_are_plain() {
        assert_eq!(item_id("asset", "user/cell-20261026120000ab12"), "asset.cell-20261026120000ab12");
        assert_eq!(item_id("asset", "user/célula-1"), "asset.c_lula-1");
        assert_eq!(item_id("template", "-x"), "template.x-x");
        assert_eq!(item_id("asset", &"a".repeat(300)).len(), "asset.".len() + 160);
    }

    #[test]
    fn names_stay_in_their_folder() {
        assert_eq!(safe_name("../../etc"), ".._.._etc");
        assert_eq!(safe_name(".."), "_");
        assert_eq!(safe_name("a.png"), "a.png");
    }

    #[test]
    fn rows_round_trip() {
        let a = Asset { id: "user/x".into(), title: "X".into(), tags: vec!["t".into()], png_local: Some("/biocartoon/user_generated/x.png".into()), generative_ai_used: Some(true), modified: 5, ..Default::default() };
        assert_eq!(Asset::from_row(&a.to_row(), 5), a);
    }
}
