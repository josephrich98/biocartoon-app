#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,ureq=warn")).init();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("BioCartoon")
        .with_app_id("com.biocartoon.app")
        .with_inner_size([1600.0, 980.0])
        .with_min_inner_size([1100.0, 700.0]);
    if let Some(icon) = biocartoon::icon_data() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, centered: true, ..Default::default() };
    eframe::run_native("BioCartoon", options, Box::new(|cc| Ok(Box::new(biocartoon::BioCartoon::new(cc)))))
}

/// Web entry point (built with `trunk serve` / `trunk build`, see index.html).
#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast;
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        biocartoon::storage::init().await;
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
