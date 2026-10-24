#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,ureq=warn")).init();
    install_crash_log();
    // Files to open: what Windows and Linux pass when a .biocartoon file is double-clicked (the
    // installers register the association). With an editor already running, it opens them instead.
    let files: Vec<std::path::PathBuf> = std::env::args_os()
        .skip(1)
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_file())
        .filter_map(|p| std::path::absolute(p).ok())
        .collect();
    if !files.is_empty() && biocartoon::control::send(&biocartoon::query::app_dir(), "open_files", serde_json::json!({ "paths": files })).is_ok() {
        return Ok(());
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("BioCartoon")
        .with_app_id("com.biocartoon.app")
        .with_inner_size([1600.0, 980.0])
        .with_min_inner_size([1100.0, 700.0]);
    if let Some(icon) = biocartoon::icon_data() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, centered: true, ..Default::default() };
    eframe::run_native(
        "BioCartoon",
        options,
        Box::new(|cc| {
            let app = biocartoon::BioCartoon::new(cc);
            app.open_paths(files);
            Ok(Box::new(app))
        }),
    )
}

/// Write panics to `<app data>/biocartoon/crash.log` and say so in a dialog: release builds on
/// Windows have no console, so a crash would otherwise close the window without a trace.
#[cfg(not(target_arch = "wasm32"))]
fn install_crash_log() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        let path = dirs::data_dir().unwrap_or_else(|| ".".into()).join("biocartoon").join("crash.log");
        let thread = std::thread::current();
        let entry = format!(
            "{} BioCartoon {} on {} (thread {}):\n{info}\n{}\n\n",
            chrono::Local::now().to_rfc3339(),
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            thread.name().unwrap_or("unnamed"),
            std::backtrace::Backtrace::force_capture(),
        );
        use std::io::Write;
        let logged = std::fs::create_dir_all(path.parent().unwrap_or(&path)).is_ok()
            && std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| f.write_all(entry.as_bytes())).is_ok();
        // Message boxes must come from the main thread on macOS; other threads' panics are logged only.
        if thread.name() == Some("main") {
            let detail = if logged { format!("The details were saved to {}.", path.display()) } else { info.to_string() };
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("BioCartoon crashed")
                .set_description(format!("Sorry, BioCartoon hit an error and has to close. Your projects are saved every few seconds, so at most the last few seconds of changes are lost.\n\n{detail}\n\nPlease report it at {}/issues.", biocartoon::GITHUB_URL))
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }
    }));
}

/// Web entry point (built with `trunk serve` / `trunk build`, see index.html).
#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast;
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        biocartoon::storage::init().await;
        biocartoon::doc::load_fonts().await;
        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("biocartoon_canvas"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("index.html has a <canvas id=\"biocartoon_canvas\">");
        let res = eframe::WebRunner::new()
            .start(canvas, eframe::WebOptions::default(), Box::new(|cc| Ok(Box::new(biocartoon::BioCartoon::new(cc)))))
            .await;
        if let Err(e) = res {
            log::error!("failed to start biocartoon: {e:?}");
        }
    });
}
