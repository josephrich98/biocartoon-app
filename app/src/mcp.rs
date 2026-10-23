//! An MCP server over stdio (`biocartoon mcp`), so an assistant such as Claude Code can
//! search the icon catalog, look at candidates, and place and arrange them on the slide of a
//! running editor (through [`crate::control`]). The catalog tools and the protocol are
//! [`biocartoon_catalog::mcp`]; this adds the template and canvas tools.

use crate::control::Client;
use crate::query;
use biocartoon_catalog::mcp::{bool_prop, image_png, int_prop, num_prop, pretty, str_prop, text, tool, CatalogTools, ServerInfo, Tools, CATALOG_INSTRUCTIONS};
pub use biocartoon_catalog::mcp::{parse_sort, PROTOCOL_VERSION};
use serde_json::{json, Map, Value};

const EDITOR_INSTRUCTIONS: &str = "BioCartoon is also a slide editor; the canvas_* tools drive the one running on this computer. \
Workflow: search_assets -> judge the results -> asset_image when unsure -> canvas_insert_asset -> canvas_state or \
canvas_render to check -> canvas_move_items / canvas_update_item to adjust.\n\
Canvas coordinates are document pixels at 96 dpi (1 in = 96 px), origin at the page's top-left; x/y are an item's \
top-left corner. canvas_state gives the page size and every item on the current slide. Edits are undoable in the editor.";

fn ids_prop() -> Value {
    json!({"type": "array", "items": {"type": "integer"}, "description": "Item ids (from canvas_state)."})
}
/// The `font` property's help: each font's snake_case name and its menu label.
fn font_help() -> String {
    let names: Vec<String> = crate::doc::TextFont::ALL
        .iter()
        .map(|f| {
            let mut snake = String::new();
            for (i, c) in format!("{f:?}").chars().enumerate() {
                if c.is_uppercase() && i > 0 {
                    snake.push('_');
                }
                snake.push(c.to_ascii_lowercase());
            }
            format!("{snake} ({})", f.label())
        })
        .collect();
    format!("Text: one of {}.", names.join(", "))
}

fn color_prop(desc: &str) -> Value {
    json!({"type": "array", "items": {"type": "integer", "minimum": 0, "maximum": 255}, "minItems": 3, "maxItems": 4, "description": format!("{desc} as [r, g, b] or [r, g, b, a], 0-255.")})
}

const GEOMETRY: [(&str, &str); 4] = [
    ("x", "Left edge in page pixels."),
    ("y", "Top edge in page pixels."),
    ("width", "Width in page pixels."),
    ("height", "Height in page pixels."),
];

fn with_geometry(mut props: Map<String, Value>) -> Value {
    for (k, d) in GEOMETRY {
        props.insert(k.into(), num_prop(d));
    }
    Value::Object(props)
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// Appearance fields `canvas_update_item` and the `insert_*` tools accept.
fn style_props() -> Map<String, Value> {
    obj(json!({
        "name": str_prop("Layer name."),
        "rotation": num_prop("Rotation in degrees, clockwise."),
        "flip_h": bool_prop("Mirror horizontally."),
        "flip_v": bool_prop("Mirror vertically."),
        "locked": bool_prop("Lock against editing in the UI."),
        "opacity": num_prop("0 (invisible) to 1."),
        "crop": {"type": "array", "items": {"type": "number"}, "minItems": 4, "maxItems": 4, "description": "Images: crop as [left, top, right, bottom] in 0..1 of the image (uncropped is [0, 0, 1, 1])."},
        "overlay": color_prop("Images: colour tint; the alpha is the strength (0 = none)"),
        "text": str_prop("Text items: the text."),
        "font_size": num_prop("Text and tables: font size in page pixels."),
        "color": color_prop("Text and lines: colour"),
        "bold": bool_prop("Text: bold."),
        "italic": bool_prop("Text: italic."),
        "underline": bool_prop("Text: underlined."),
        "font": str_prop(&font_help()),
        "line_spacing": num_prop("Text: multiple of the natural line height (1, 1.15, 1.5, 2...)."),
        "align": str_prop("Text: left, center, right or justify."),
        "valign": str_prop("Text: where the text sits in its box: top, middle or bottom."),
        "shape": str_prop("Shapes: rectangle, rounded_rectangle, ellipse, triangle, diamond, hexagon, star, block_arrow."),
        "fill": color_prop("Shapes: fill"),
        "stroke": color_prop("Shapes: outline colour"),
        "stroke_width": num_prop("Shapes: outline width."),
        "line_kind": str_prop("Lines: straight, arrow, reverse_arrow, double_arrow, inhibitor, equilibrium, reverse_equilibrium, bent, bent_arrow, bent_reverse_arrow, bent_double_arrow, bent_inhibitor, spline, spline_arrow, spline_reverse_arrow, spline_double_arrow, circular, circular_arrow, circular_reverse_arrow, circular_double_arrow."),
        "line_style": str_prop("Lines: solid, dashed or dotted."),
        "line_width": num_prop("Lines: stroke width."),
        "x1": num_prop("Lines: start x."), "y1": num_prop("Lines: start y."),
        "x2": num_prop("Lines: end x."), "y2": num_prop("Lines: end y."),
        "cells": {"type": "array", "items": {"type": "string"}, "description": "Tables: cell texts, row-major."},
        "code": str_prop("Code blocks: the source code."),
        "language": str_prop("Code blocks: auto (detect), plain, python, r, julia, matlab, rust, c, cpp, java, javascript, typescript, go, bash, sql, json or yaml."),
        "theme": str_prop("Code blocks: dracula, one_dark, monokai, nord, github_light or solarized_light."),
        "line_numbers": {"type": "boolean", "description": "Code blocks: show line numbers."},
        "window": {"type": "boolean", "description": "Code blocks: show the title bar with window dots."},
        "rounded": {"type": "boolean", "description": "Code blocks: rounded corners (default square)."},
    }))
}

/// The editor's own tools (templates and canvas), without the catalog's.
fn editor_tools() -> Vec<Value> {
    let mut insert_asset = obj(json!({
        "id": str_prop("Catalog asset id (from search_assets), e.g. bioart/424_633913."),
        "replace_item": int_prop("Swap this existing image item's picture for the asset instead of adding a new item."),
    }));
    insert_asset.extend(style_props());
    let mut insert_text = obj(json!({"text": str_prop("The text (may contain newlines).")}));
    insert_text.extend(style_props());
    let mut insert_code = obj(json!({}));
    insert_code.extend(style_props());
    let mut insert_shape = obj(json!({"shape": str_prop("rectangle, rounded_rectangle, ellipse, triangle, diamond, hexagon, star or block_arrow.")}));
    insert_shape.extend(style_props());
    let mut insert_line = obj(json!({"line_kind": str_prop("straight, arrow, reverse_arrow, double_arrow, inhibitor, equilibrium, reverse_equilibrium, bent, bent_arrow, bent_reverse_arrow, bent_double_arrow, bent_inhibitor, spline, spline_arrow, spline_reverse_arrow, spline_double_arrow, circular, circular_arrow, circular_reverse_arrow, circular_double_arrow (default arrow)."), "line_style": str_prop("solid, dashed or dotted (default solid).")}));
    insert_line.extend(style_props());
    let mut update = obj(json!({"id": int_prop("The item to change.")}));
    update.extend(style_props());
    vec![
        tool("list_templates", "Slide templates: the editor's built-in layouts (builtin:N) and saved / shared templates. Optional word filter.", json!({"query": str_prop("Words that must appear in the title, description or category.")}), &[]),
        tool("template_image", "The thumbnail of a saved or shared template (built-in layouts have none).", json!({"id": str_prop("Template id.")}), &["id"]),
        tool("canvas_state", "What is open in the editor: page size, current slide and every item on it (ids, type, position, size, asset). Call it before editing and after inserting.", json!({}), &[]),
        tool("canvas_render", "A picture of a slide as it looks now (PNG), to check the layout visually.", json!({"slide": int_prop("Slide index (default: the current one)."), "max_side": int_prop("Longest side in pixels (default 1024).")}), &[]),
        tool("canvas_insert_asset", "Place a catalog asset on the current slide (centred unless x/y given; sized to a quarter of the page unless width/height given). Returns the new item, including its id.", with_geometry(insert_asset), &["id"]),
        tool("canvas_insert_template", "Add a template's slide after the current one, scaled to the page (builtin:N layouts or a saved / shared template id from list_templates).", json!({"id": str_prop("Template id.")}), &["id"]),
        tool("canvas_insert_text", "Add a text box.", with_geometry(insert_text), &["text"]),
        tool("canvas_insert_shape", "Add a shape.", with_geometry(insert_shape), &[]),
        tool("canvas_insert_code", "Add a syntax-highlighted code block, sized to fit the code unless width/height given.", with_geometry(insert_code), &["code"]),
        tool("canvas_insert_line", "Add a line or arrow (x1/y1 to x2/y2, or centred by default).", with_geometry(insert_line), &[]),
        tool("canvas_update_item", "Change an item's geometry or appearance (any subset of the fields). Returns the item.", with_geometry(update), &["id"]),
        tool("canvas_move_items", "Move items: absolute x/y places the group's top-left corner, dx/dy shifts them.", json!({"ids": ids_prop(), "x": num_prop("New left edge."), "y": num_prop("New top edge."), "dx": num_prop("Shift right (negative: left)."), "dy": num_prop("Shift down (negative: up).")}), &["ids"]),
        tool("canvas_delete_items", "Remove items from the slide.", json!({"ids": ids_prop()}), &["ids"]),
        tool("canvas_select", "Select items in the editor (an empty list clears the selection).", json!({"ids": ids_prop()}), &["ids"]),
        tool("canvas_duplicate", "Duplicate items (the copies are returned and selected).", json!({"ids": ids_prop()}), &["ids"]),
        tool("canvas_align", "Align or distribute items: left, center_h, right, top, middle, bottom, distribute_h, distribute_v. One item aligns to the page.", json!({"kind": str_prop("Alignment."), "ids": ids_prop()}), &["kind"]),
        tool("canvas_arrange", "Change stacking order: front, forward, backward, back.", json!({"kind": str_prop("Order change."), "ids": ids_prop()}), &["kind", "ids"]),
        tool("canvas_slides", "List the document's slides.", json!({}), &[]),
        tool("canvas_go_slide", "Switch to a slide.", json!({"index": int_prop("0-based slide index.")}), &["index"]),
        tool("canvas_add_slide", "Add an empty slide after the current one and switch to it.", json!({}), &[]),
        tool("canvas_set_page", "Change the page size or background.", json!({"width": num_prop("Page width in `unit`."), "height": num_prop("Page height in `unit`."), "unit": str_prop("inch, cm or px (default: the document's)."), "background": color_prop("Page colour")}), &[]),
        tool("canvas_undo", "Undo the last edit.", json!({}), &[]),
        tool("canvas_redo", "Redo.", json!({}), &[]),
        tool("canvas_save", "Save the document as a project in the editor.", json!({}), &[]),
    ]
}

/// The catalog's tools plus the editor's. The editor's template tools replace the catalog's
/// (which list only the shared library) with ones that also see built-in and own templates.
pub fn tools() -> Vec<Value> {
    let editor = editor_tools();
    let mut ts: Vec<Value> = CatalogTools::default().tools().into_iter().filter(|t| !editor.iter().any(|e| e["name"] == t["name"])).collect();
    ts.extend(editor);
    ts
}

struct EditorTools {
    catalog: CatalogTools,
    app_dir: std::path::PathBuf,
}

impl Tools for EditorTools {
    fn tools(&self) -> Vec<Value> {
        tools()
    }

    fn call(&mut self, name: &str, args: &Value) -> Option<Result<Vec<Value>, String>> {
        if editor_tools().iter().any(|t| t["name"] == name) {
            return Some(self.run(name, args));
        }
        self.catalog.call(name, args)
    }
}

impl EditorTools {
    fn canvas(&self, cmd: &str, args: Value) -> Result<Value, String> {
        Client::connect(&self.app_dir)?.call(cmd, args)
    }

    fn run(&mut self, name: &str, args: &Value) -> Result<Vec<Value>, String> {
        let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
        let n = |k: &str| args.get(k).and_then(Value::as_u64);
        let need = |k: &str| s(k).ok_or_else(|| format!("{k} is required"));
        Ok(match name {
            "list_templates" => vec![pretty(&query::templates(&s("query").unwrap_or_default()))],
            "template_image" => {
                let id = need("id")?;
                let t = query::templates("").into_iter().find(|t| t.id == id).ok_or_else(|| format!("no template {id}"))?;
                let url = t.thumb_url.ok_or("this template has no thumbnail")?;
                let req = if std::path::Path::new(&url).exists() { crate::assets::Request::Path(url.into()) } else { crate::assets::Request::Url(url) };
                let (png, _, _) = query::shrink_png(&crate::assets::fetch(&req)?, 512)?;
                vec![text(format!("{} — {}", t.title, t.description)), image_png(&png)]
            }
            "canvas_state" => vec![pretty(&self.canvas("state", json!({}))?)],
            "canvas_render" => {
                let mut a = json!({"max_side": n("max_side").unwrap_or(1024)});
                if let Some(i) = n("slide") {
                    a["slide"] = json!(i);
                }
                let r = self.canvas("render", a)?;
                let b64 = r.get("png_base64").and_then(Value::as_str).ok_or("no image")?;
                vec![text(format!("Slide rendered at {}x{} px.", r["width"], r["height"])), json!({"type": "image", "data": b64, "mimeType": "image/png"})]
            }
            "canvas_insert_asset" => {
                let id = need("id")?;
                let a = self.catalog.catalog()?.get(&id)?.ok_or_else(|| format!("no asset {id}"))?;
                let mut req = args.clone();
                req["asset"] = serde_json::to_value(&a).map_err(|e| e.to_string())?;
                vec![pretty(&self.canvas("insert_asset", req)?)]
            }
            "canvas_slides" | "canvas_add_slide" | "canvas_undo" | "canvas_redo" | "canvas_save" => {
                vec![pretty(&self.canvas(&name["canvas_".len()..], json!({}))?)]
            }
            // The rest pass their arguments straight through: canvas_<cmd> runs <cmd>.
            other => vec![pretty(&self.canvas(&other["canvas_".len()..], args.clone())?)],
        })
    }
}

fn server_info() -> ServerInfo {
    ServerInfo { name: "biocartoon", version: env!("CARGO_PKG_VERSION"), instructions: format!("{CATALOG_INSTRUCTIONS}\n{EDITOR_INSTRUCTIONS}") }
}

/// Serve MCP on stdin/stdout until the client closes the pipe.
pub fn serve() -> Result<(), String> {
    biocartoon_catalog::mcp::serve(&server_info(), &mut EditorTools { catalog: CatalogTools::default(), app_dir: query::app_dir() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use biocartoon_catalog::mcp::handle;

    #[test]
    fn tool_schemas_are_well_formed() {
        let ts = tools();
        assert!(ts.len() > 25);
        biocartoon_catalog::mcp::check_tool_schemas(&ts);
    }

    #[test]
    fn initialize_and_unknown_method() {
        let mut s = EditorTools { catalog: CatalogTools::default(), app_dir: std::env::temp_dir() };
        let info = server_info();
        let r = handle(&info, &mut s, "initialize", &json!({"protocolVersion": "2025-03-26"})).unwrap();
        assert_eq!(r["protocolVersion"], "2025-03-26");
        assert!(r["instructions"].as_str().unwrap().contains("search_assets"));
        assert_eq!(handle(&info, &mut s, "initialize", &json!({"protocolVersion": "1999-01-01"})).unwrap()["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(handle(&info, &mut s, "nope", &json!({})).unwrap_err().0, -32601);
        // Canvas tools without a running editor fail as a tool error, not a protocol error.
        let r = handle(&info, &mut s, "tools/call", &json!({"name": "canvas_state"})).unwrap();
        assert_eq!(r["isError"], true);
        let r = handle(&info, &mut s, "tools/call", &json!({"name": "canvas_nope"})).unwrap();
        assert!(r["content"][0]["text"].as_str().unwrap().contains("unknown tool"));
    }
}
