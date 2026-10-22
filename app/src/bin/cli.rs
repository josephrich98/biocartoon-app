//! `biocartoon`: search the icon catalog and templates from a shell, drive a running editor, or
//! serve both to an assistant over MCP (`biocartoon mcp`). Desktop only. The catalog commands are
//! `biocartoon-catalog`'s ([`biocartoon_catalog::cli`]); this adds templates and the editor.

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    if let Err(e) = cli::run(std::env::args().skip(1).collect()) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod cli {
    use biocartoon::query::{self, Catalog};
    use biocartoon::{control, mcp};
    use biocartoon_catalog::cli::{command_help, print_json, run_command, version, Args, CATALOG_COMMANDS, CATALOG_OPTIONS};
    use serde_json::{json, Value};

    fn usage() -> String {
        format!(
            "\
biocartoon — search the BioCartoon icon catalog and templates, and drive the editor

USAGE
  biocartoon <command> [arguments] [options]

{CATALOG_COMMANDS}
EDITOR (the desktop app must be running)
  app <cmd> [json]          send a command, e.g.
                              app state
                              app move_items '{{\"ids\":[3],\"dx\":48}}'
                              app insert_asset '{{\"asset\":\"bioart/424_633918\",\"x\":200,\"width\":300}}'
                              app render '{{\"path\":\"slide.png\"}}'
                            commands: state get_item slides go_slide add_slide new_document set_page
                            insert_asset insert_text insert_shape insert_line insert_code insert_template
                            update_item move_items delete_items select duplicate align arrange
                            undo redo save render

ASSISTANTS
  mcp                       serve the catalog and editor tools over MCP on stdin/stdout:
                              claude mcp add biocartoon -- biocartoon mcp

{CATALOG_OPTIONS}"
        )
    }

    fn json_arg(a: &Args, i: usize) -> Result<Value, String> {
        match a.pos.get(i) {
            Some(j) => serde_json::from_str(j).map_err(|e| format!("arguments must be JSON: {e}")),
            None => Ok(json!({})),
        }
    }

    /// The editor's templates matching `q`, for `search`.
    fn templates(q: &str) -> Vec<Value> {
        query::templates(q).into_iter().filter_map(|t| serde_json::to_value(t).ok()).collect()
    }

    pub fn run(argv: Vec<String>) -> Result<(), String> {
        let a = Args::parse(argv, &["slide"]);
        a.apply_env();
        if a.wants_help() {
            print!("{}", a.help_topic().and_then(command_help).unwrap_or_else(usage));
            return Ok(());
        }
        if a.flag("version") {
            return version(&a, env!("CARGO_PKG_VERSION"));
        }
        match a.command() {
            "mcp" => mcp::serve(),
            "app" => {
                let c = a.pos.get(1).ok_or("app <cmd> [json]")?;
                let mut args = json_arg(&a, 2)?;
                // `insert_asset` takes an asset id in place of the whole catalog row.
                if c == "insert_asset" {
                    if let Some(id) = args.get("asset").and_then(Value::as_str).map(String::from) {
                        let asset = Catalog::open_default()?.get(&id)?.ok_or_else(|| format!("no asset {id}"))?;
                        args["asset"] = serde_json::to_value(&asset).map_err(|e| e.to_string())?;
                    }
                }
                print_json(&control::send(&query::app_dir(), c, args)?);
                Ok(())
            }
            other => run_command(&a, Some(&templates)).unwrap_or_else(|| Err(format!("unknown command {other:?}\n\n{}", usage()))),
        }
    }
}
