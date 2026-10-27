//! Image loading: fetches PNGs from the R2 bucket (or local files), decodes them on worker
//! threads and turns them into egui textures on the UI thread. LRU-evicts GPU textures.
//!
//! Pending loads are a priority queue rather than a FIFO: each frame, images are ranked in the
//! order the UI asked for them, so a grid fills in reading order and images that are no longer
//! drawn (e.g. after switching tabs) are dropped instead of delaying the visible ones.
//!
//! SVGs stay vector: the parsed tree is kept next to the base texture, and the canvas asks for
//! a rendering at the resolution it is about to draw ([`ImageCache::lod_texture`]). Those
//! renderings are cached per power-of-two size and evicted by a byte budget, so an SVG asset is
//! sharp at any zoom while a PNG-only asset is drawn as the pixels it has.

use crate::doc::LayerStyle;
use crate::export::Vector;
use egui::{ColorImage, TextureHandle, TextureOptions};
use image::RgbaImage;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, RwLock};
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

/// Longest side kept in GPU memory. Larger images are downscaled on the worker thread.
pub(crate) const MAX_SIDE: u32 = 768;
/// Smallest and largest longer side of an SVG rendered for the canvas beyond the base texture.
/// Sizes go up by powers of two in between, so a drawing is never minified more than 2x.
const MIN_LOD_SIDE: u32 = 1024;
const MAX_LOD_SIDE: u32 = 4096;
/// GPU memory kept for those renderings before the least recently drawn are dropped.
const LOD_BUDGET_BYTES: usize = 256 * 1024 * 1024;

/// File types accepted as uploads (picked or dropped on the canvas).
pub const IMAGE_EXTS: [&str; 7] = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "svg"];
const MAX_TEXTURES: usize = 400;

#[derive(Clone, Debug)]
pub enum Request {
    /// Fetch `<base_url>/<key>`.
    Url(String),
    /// Read from disk.
    Path(PathBuf),
    /// The first, or the second if it fails (an asset's SVG, then its PNG).
    Either(Box<Request>, Box<Request>),
}

pub enum ImgState {
    Loading,
    Ready { tex: TextureHandle, size: [usize; 2] },
    Failed(String),
}

/// What a worker does for a cache key.
enum Work {
    /// Fetch and decode an image file.
    Fetch(Request),
    /// Rasterise an SVG that is already loaded, `side` pixels on its longer side.
    Render(Arc<Vector>, u32),
    /// Restyle the layers of an SVG that is already loaded (see [`Vector::restyled`]), keeping
    /// its vector form as a loaded file's.
    Restyle(Arc<Vector>, Vec<LayerStyle>),
}

/// A decoded image, and its vector form when the file was an SVG.
pub struct Loaded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub svg: Option<Arc<Vector>>,
}

struct Job {
    work: Work,
    /// Frame the UI last asked for it, and its position among that frame's requests.
    frame: u64,
    order: usize,
}

/// Loads not yet picked up by a worker, keyed by cache key.
#[derive(Default)]
struct Queue {
    jobs: Mutex<HashMap<String, Job>>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    ready: Condvar,
}

impl Queue {
    /// Take the job wanted most recently, earliest in that frame.
    fn best(jobs: &mut HashMap<String, Job>) -> Option<(String, Work)> {
        let key = jobs.iter().max_by_key(|(_, j)| (j.frame, std::cmp::Reverse(j.order))).map(|(k, _)| k.clone())?;
        let job = jobs.remove(&key)?;
        Some((key, job.work))
    }

    /// Block until there is a job.
    #[cfg(not(target_arch = "wasm32"))]
    fn take(&self) -> (String, Work) {
        let mut jobs = self.jobs.lock().unwrap();
        loop {
            if let Some(j) = Self::best(&mut jobs) {
                return j;
            }
            jobs = self.ready.wait(jobs).unwrap();
        }
    }
}

/// Web build: fetches started at once, standing in for the desktop's 6 loader threads.
#[cfg(target_arch = "wasm32")]
const WEB_FETCHES: usize = 6;

/// Queued loads not asked for in this many frames are dropped (re-queued if asked for again).
const STALE_FRAMES: u64 = 2;

type Decoded = (String, Result<Loaded, String>);

pub struct ImageCache {
    map: HashMap<String, ImgState>,
    /// Decoded CPU copies, kept for the AI edit operations and export.
    cpu: HashMap<String, Arc<RgbaImage>>,
    /// Parsed SVGs, by the cache key of their base texture, for re-rendering at any size.
    svg: HashMap<String, Arc<Vector>>,
    /// The last restyling of each SVG that was ready (see [`ImageCache::restyled`]), by its cache key.
    last_restyled: HashMap<String, String>,
    /// Bytes of GPU memory held by the `lod:` renderings in `map`.
    lod_bytes: usize,
    lru: VecDeque<String>,
    queue: Arc<Queue>,
    /// Keys still loading that the UI asked for this frame, in request order.
    wanted: Vec<String>,
    frame: u64,
    rx: Receiver<Decoded>,
    /// The web build has no loader threads: fetch callbacks send decoded images here.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    dtx: Sender<Decoded>,
    /// Web build: fetches started and not yet delivered to `rx`.
    #[cfg(target_arch = "wasm32")]
    fetching: usize,
    pub base_url: String,
    pub local_dir: Option<PathBuf>,
    /// Desktop: the offline folder's `assets/` (see [`crate::offline`]). PNGs fetched from the
    /// network are kept there, and read from there first.
    cache_dir: Arc<RwLock<Option<PathBuf>>>,
    pub loads_in_flight: usize,
}

pub fn fetch(req: &Request) -> Result<Vec<u8>, String> {
    fetch_first(req).map(|(bytes, _)| bytes)
}

/// [`fetch`], and whether the bytes came from the network as the request's first choice (so
/// they are the asset its cache key names, worth keeping for offline use).
fn fetch_first(req: &Request) -> Result<(Vec<u8>, bool), String> {
    match req {
        Request::Either(a, b) => fetch_first(a).or_else(|e| fetch(b).map(|bytes| (bytes, false)).map_err(|e2| format!("{e}; {e2}"))),
        Request::Url(_) => fetch_one(req).map(|bytes| (bytes, true)),
        Request::Path(_) => fetch_one(req).map(|bytes| (bytes, false)),
    }
}

fn fetch_one(req: &Request) -> Result<Vec<u8>, String> {
    match req {
        Request::Either(..) => fetch(req),
        Request::Path(p) => crate::storage::read(p).map_err(|e| format!("{}: {e}", p.display())),
        #[cfg(target_arch = "wasm32")]
        Request::Url(url) => Err(format!("GET {url}: blocking fetches are not available in the web build")),
        #[cfg(not(target_arch = "wasm32"))]
        Request::Url(url) => {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(30)))
                .build()
                .into();
            let mut res = agent.get(url).call().map_err(|e| format!("GET {url}: {e}"))?;
            res.body_mut()
                .with_config()
                .limit(64 * 1024 * 1024)
                .read_to_vec()
                .map_err(|e| format!("read {url}: {e}"))
        }
    }
}

/// Web build: [`fetch`] without blocking; `done` gets the bytes.
#[cfg(target_arch = "wasm32")]
pub fn fetch_web(req: Request, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
    match req {
        Request::Path(_) => done(fetch(&req)),
        Request::Url(url) => ehttp::fetch(ehttp::Request::get(&url), move |res| match res {
            // A network error here is usually CORS: the browser reused a copy of the asset that
            // biocartoon.com's landing page loaded without CORS headers (the two sites share the
            // HTTP cache, and assets are cached as immutable for a year). The asset bucket
            // ignores the query, so asking again with one gets a fresh copy past that entry.
            Err(_) => {
                let retry = format!("{url}{}cors=1", if url.contains('?') { '&' } else { '?' });
                ehttp::fetch(ehttp::Request::get(&retry), move |res| done(web_body(&url, res)))
            }
            res => done(web_body(&url, res)),
        }),
        Request::Either(a, b) => fetch_web(*a, Box::new(move |res| match res {
            Ok(bytes) => done(Ok(bytes)),
            Err(e) => fetch_web(*b, Box::new(move |res| done(res.map_err(|e2| format!("{e}; {e2}"))))),
        })),
    }
}

/// Web build: a fetch's body, or why there is none.
#[cfg(target_arch = "wasm32")]
fn web_body(url: &str, res: ehttp::Result<ehttp::Response>) -> Result<Vec<u8>, String> {
    let r = res.map_err(|e| format!("GET {url}: {e}"))?;
    if r.ok { Ok(r.bytes) } else { Err(format!("GET {url}: {} {}", r.status, r.status_text)) }
}

/// Web build: [`fetch_decode`] without blocking; `done` gets the decoded image.
#[cfg(target_arch = "wasm32")]
fn fetch_decode_web(req: Request, done: Box<dyn FnOnce(Result<Loaded, String>) + Send>) {
    match req {
        Request::Either(a, b) => fetch_decode_web(*a, Box::new(move |res| match res {
            Ok(l) => done(Ok(l)),
            Err(e) => fetch_decode_web(*b, Box::new(move |res| done(res.map_err(|e2| format!("{e}; {e2}"))))),
        })),
        req => fetch_web(req, Box::new(move |bytes| done(bytes.and_then(|b| decode(&b))))),
    }
}

/// Whether `bytes` are an SVG (plain or gzipped) rather than a raster image. The root may carry
/// a namespace prefix (`<ns0:svg`, as Python's ElementTree writes it), not only `<svg`. A file
/// with a raster signature is never one, even when its metadata embeds an SVG (ChatGPT PNGs
/// carry a C2PA manifest with an SVG logo in its first few hundred bytes).
pub fn is_svg(bytes: &[u8]) -> bool {
    if image::guess_format(bytes).is_ok() {
        return false;
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return true;
    }
    let head = &bytes[..bytes.len().min(4096)];
    head.iter().enumerate().filter(|(_, b)| **b == b'<').any(|(i, _)| {
        // The tag name, with any `prefix:` stripped.
        let name = head[i + 1..].iter().take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'));
        let name: Vec<u8> = name.copied().collect();
        name.rsplit(|b| *b == b':').next() == Some(b"svg")
    })
}

/// True for a file path on this computer rather than an R2 key: user uploads store those.
/// (wasm32's `is_absolute` is false even for "/..."; app data there lives under "/biocartoon".)
pub fn is_local_path(p: &str) -> bool {
    std::path::Path::new(p).is_absolute() || (cfg!(target_arch = "wasm32") && p.starts_with('/'))
}

/// Decode an image file. SVGs are rasterised to fit `svg_side` pixels, since everything
/// downstream (canvas textures, exports, AI edits) works on pixels.
pub fn load_image(bytes: &[u8], svg_side: u32) -> Result<RgbaImage, String> {
    if is_svg(bytes) {
        crate::export::render_svg(bytes, svg_side)
    } else {
        image::load_from_memory(bytes).map(|i| i.to_rgba8()).map_err(|e| e.to_string())
    }
}

/// An image file's size in pixels (an SVG's in user units), without decoding it.
pub fn image_size(bytes: &[u8]) -> Option<(f32, f32)> {
    if is_svg(bytes) {
        return crate::export::svg_size(bytes);
    }
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok()?;
    Some((w as f32, h as f32))
}

/// Decode an image file for the canvas: pixels at most `MAX_SIDE` on the longer side, plus the
/// parsed file when it is an SVG (so it can be rendered larger later).
pub fn decode(bytes: &[u8]) -> Result<Loaded, String> {
    let (img, svg) = if is_svg(bytes) {
        let vector = Vector::parse(bytes)?;
        (vector.render(MAX_SIDE)?, Some(Arc::new(vector)))
    } else {
        (image::load_from_memory(bytes).map(|i| i.to_rgba8()).map_err(|e| e.to_string())?, None)
    };
    let (w, h) = img.dimensions();
    let m = w.max(h);
    let img = if m > MAX_SIDE {
        let s = MAX_SIDE as f32 / m as f32;
        let nw = ((w as f32 * s) as u32).max(1);
        let nh = ((h as f32 * s) as u32).max(1);
        image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    Ok(loaded(img, svg))
}

fn loaded(img: RgbaImage, svg: Option<Arc<Vector>>) -> Loaded {
    let (width, height) = img.dimensions();
    Loaded { width, height, rgba: img.into_raw(), svg }
}

/// Do a job: fetch and decode, or render an SVG at a size. `from_url` is set when the bytes
/// came from the network as the request's first choice (see [`fetch_first`]).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn run(work: &Work) -> Result<(Loaded, Option<Vec<u8>>), String> {
    match work {
        Work::Fetch(req) => fetch_decode(req),
        Work::Render(vector, side) => Ok((loaded(vector.render(*side)?, None), None)),
        Work::Restyle(vector, layers) => {
            let v = vector.restyled(layers).ok_or("svg: could not restyle its layers")?;
            Ok((loaded(v.render(MAX_SIDE)?, Some(Arc::new(v))), None))
        }
    }
}

/// Fetch and decode `req`. An either-request falls back to its second choice when the first
/// fails to decode, not only when it fails to fetch: a catalog SVG the parser rejects still
/// shows as its PNG.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn fetch_decode(req: &Request) -> Result<(Loaded, Option<Vec<u8>>), String> {
    match req {
        Request::Either(a, b) => fetch_decode(a).or_else(|e| fetch_decode(b).map(|(l, _)| (l, None)).map_err(|e2| format!("{e}; {e2}"))),
        _ => {
            let (bytes, from_url) = fetch_first(req)?;
            let l = decode(&bytes)?;
            Ok((l, from_url.then_some(bytes)))
        }
    }
}

/// Cache key of `key`'s SVG rendered `side` pixels on the longer side.
fn lod_key(key: &str, side: u32) -> String {
    format!("lod:{side}:{key}")
}

fn is_lod_key(key: &str) -> bool {
    key.starts_with("lod:")
}

/// Cache key of `key`'s SVG with `layers` restyled.
fn restyle_key(key: &str, layers: &[LayerStyle]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{layers:?}").hash(&mut h);
    format!("layers:{:016x}:{key}", h.finish())
}

fn is_restyle_key(key: &str) -> bool {
    key.starts_with("layers:")
}

/// The rendering size for drawing an SVG `need` pixels on its longer side: the next power of
/// two from `MIN_LOD_SIDE`, capped by `MAX_LOD_SIDE` and the GPU's limit. `None` when the base
/// texture is enough.
fn lod_side(need: f32, max_texture_side: usize) -> Option<u32> {
    if need.is_nan() || need <= MAX_SIDE as f32 {
        return None;
    }
    let cap = MAX_LOD_SIDE.min(max_texture_side as u32);
    if cap < MIN_LOD_SIDE {
        return None;
    }
    let mut side = MIN_LOD_SIDE;
    while (side as f32) < need && side < cap {
        side = (side * 2).min(cap);
    }
    Some(side)
}

impl ImageCache {
    pub fn new(base_url: String, local_dir: Option<PathBuf>) -> Self {
        let queue = Arc::new(Queue::default());
        let (dtx, rx) = channel::<Decoded>();
        let cache_dir: Arc<RwLock<Option<PathBuf>>> = Arc::default();
        #[cfg(not(target_arch = "wasm32"))]
        for i in 0..6 {
            let queue = queue.clone();
            let dtx = dtx.clone();
            let cache_dir = cache_dir.clone();
            thread::Builder::new()
                .name(format!("img-loader-{i}"))
                .spawn(move || loop {
                    let (key, work) = queue.take();
                    match &work {
                        Work::Fetch(req) => log::debug!("loading {req:?}"),
                        Work::Render(_, side) => log::debug!("rendering {key} at {side} px"),
                        Work::Restyle(..) => log::debug!("restyling {key}"),
                    }
                    let res = run(&work).map(|(l, fetched)| {
                        if let (Some(b), Some(dir), Some(k)) = (fetched, cache_dir.read().unwrap().clone(), key.strip_prefix("asset:")) {
                            crate::offline::store(&dir, k, &b);
                        }
                        l
                    });
                    if dtx.send((key, res)).is_err() {
                        break;
                    }
                })
                .expect("spawn image loader");
        }
        Self {
            map: HashMap::new(),
            cpu: HashMap::new(),
            svg: HashMap::new(),
            last_restyled: HashMap::new(),
            lod_bytes: 0,
            lru: VecDeque::new(),
            queue,
            wanted: Vec::new(),
            frame: 0,
            rx,
            dtx,
            #[cfg(target_arch = "wasm32")]
            fetching: 0,
            base_url,
            local_dir,
            cache_dir,
            loads_in_flight: 0,
        }
    }

    pub fn configure(&mut self, base_url: String, local_dir: Option<PathBuf>) {
        if base_url != self.base_url || local_dir != self.local_dir {
            self.base_url = base_url;
            self.local_dir = local_dir;
            // Drop failures so they get retried with the new configuration.
            self.map.retain(|_, s| !matches!(s, ImgState::Failed(_)));
        }
    }

    /// Keep fetched PNGs in `dir` and load from it first.
    pub fn set_cache_dir(&self, dir: PathBuf) {
        *self.cache_dir.write().unwrap() = Some(dir);
    }

    pub fn cache_dir(&self) -> Option<PathBuf> {
        self.cache_dir.read().unwrap().clone()
    }

    /// Retry images that failed to load (e.g. after going offline and downloading a pack).
    pub fn retry_failed(&mut self) {
        self.map.retain(|_, s| !matches!(s, ImgState::Failed(_)));
    }

    fn request_for_asset(&self, key: &str, local_rel: Option<&str>) -> Result<Request, String> {
        if let Some(rel) = local_rel {
            // User uploads store an absolute path; catalog assets are relative to the data dir.
            let p = match &self.local_dir {
                _ if is_local_path(rel) => Some(std::path::PathBuf::from(rel)),
                Some(dir) => Some(dir.join(rel)),
                None => None,
            };
            if let Some(p) = p.filter(|p| crate::storage::exists(p)) {
                return Ok(Request::Path(p));
            }
        }
        if let Some(p) = self.cache_dir().and_then(|d| crate::offline::cache_path(&d, key)).filter(|p| p.is_file()) {
            return Ok(Request::Path(p));
        }
        if !self.base_url.is_empty() {
            return Ok(Request::Url(format!("{}/{}", self.base_url.trim_end_matches('/'), key)));
        }
        Err("No asset URL configured.".into())
    }

    /// Where to load a catalog image from: its SVG when it has one, else (or if that fails, as
    /// when offline with only the PNGs downloaded) its PNG.
    fn request_for_image(&self, key: &str, svg: Option<&str>, local_rel: Option<&str>) -> Result<Request, String> {
        let png = self.request_for_asset(key, local_rel);
        // The user's own SVGs (drawings an AI model made) are on this computer, by absolute path.
        match (svg.map(|k| self.request_for_asset(k, is_local_path(k).then_some(k))), png) {
            (Some(Ok(svg)), Ok(png)) => Ok(Request::Either(Box::new(svg), Box::new(png))),
            (Some(Ok(svg)), Err(_)) => Ok(svg),
            (_, png) => png,
        }
    }

    /// Where to load the full-resolution original of a source (used by export): the offline
    /// copy when there is one, else the asset URL.
    pub fn request_for(&self, src: &crate::doc::ImageSource) -> Result<Request, String> {
        match src {
            crate::doc::ImageSource::Asset { key, svg, .. } => self.request_for_image(key, svg.as_deref(), None),
            crate::doc::ImageSource::File { path, .. } => Ok(Request::Path(PathBuf::from(path))),
        }
    }

    /// Ask for an asset PNG by R2 key. Returns the current state.
    pub fn asset(&mut self, key: &str, local_rel: Option<&str>) -> &ImgState {
        self.asset_image(key, None, local_rel)
    }

    /// Ask for a catalog image placed on the canvas: its SVG (`svg`, an R2 key) when it has one,
    /// with the PNG (`key`) as the fallback. Cached under the key of the one it prefers, as
    /// [`crate::doc::ImageSource::cache_key`] names it.
    pub fn asset_image(&mut self, key: &str, svg: Option<&str>, local_rel: Option<&str>) -> &ImgState {
        let ck = format!("asset:{}", svg.unwrap_or(key));
        if !self.map.contains_key(&ck) {
            match self.request_for_image(key, svg, local_rel) {
                Ok(req) => self.queue(ck.clone(), Work::Fetch(req)),
                Err(e) => {
                    self.map.insert(ck.clone(), ImgState::Failed(e));
                }
            }
        } else {
            self.want(&ck);
        }
        self.touch(&ck);
        self.map.get(&ck).unwrap()
    }

    pub fn file(&mut self, path: &str) -> &ImgState {
        let ck = format!("file:{path}");
        if !self.map.contains_key(&ck) {
            self.queue(ck.clone(), Work::Fetch(Request::Path(PathBuf::from(path))));
        } else {
            self.want(&ck);
        }
        self.touch(&ck);
        self.map.get(&ck).unwrap()
    }

    /// Look up by cache key without triggering a load.
    pub fn get(&self, cache_key: &str) -> Option<&ImgState> {
        self.map.get(cache_key)
    }

    pub fn cpu_image(&self, cache_key: &str) -> Option<Arc<RgbaImage>> {
        self.cpu.get(cache_key).cloned()
    }

    /// A loaded SVG's vector form (its artboard and drawing), if the key is one.
    pub fn vector(&self, cache_key: &str) -> Option<Arc<Vector>> {
        self.svg.get(cache_key).cloned()
    }

    /// Replace the cached texture for a key with a new image (after an AI edit) and return it.
    pub fn insert_image(&mut self, ctx: &egui::Context, cache_key: &str, img: RgbaImage) {
        let (w, h) = img.dimensions();
        let ci = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
        let tex = ctx.load_texture(cache_key, ci, TextureOptions::LINEAR);
        self.cpu.insert(cache_key.to_string(), Arc::new(img));
        self.map.insert(cache_key.to_string(), ImgState::Ready { tex, size: [w as usize, h as usize] });
        self.forget_svg(cache_key);
        self.touch(cache_key);
    }

    /// [`insert_image`](Self::insert_image) for an SVG file's bytes, keeping its vector form so
    /// it is drawn sharp at any zoom.
    pub fn insert_svg(&mut self, ctx: &egui::Context, cache_key: &str, bytes: &[u8]) -> Result<(), String> {
        let Loaded { width, height, rgba, svg } = decode(bytes)?;
        let img = RgbaImage::from_raw(width, height, rgba).ok_or("svg: bad rendering size")?;
        self.insert_image(ctx, cache_key, img);
        if let Some(vector) = svg {
            self.svg.insert(cache_key.to_string(), vector);
        }
        Ok(())
    }

    pub fn forget(&mut self, cache_key: &str) {
        self.map.remove(cache_key);
        self.cpu.remove(cache_key);
        self.forget_svg(cache_key);
    }

    /// Drop a key's vector form and every rendering and restyling made from it.
    fn forget_svg(&mut self, cache_key: &str) {
        self.svg.remove(cache_key);
        let suffix = format!(":{cache_key}");
        let restyled: Vec<String> = self.map.keys().filter(|k| is_restyle_key(k) && k.ends_with(&suffix)).cloned().collect();
        for k in restyled {
            self.forget(&k);
        }
        self.last_restyled.remove(cache_key);
        let lods: Vec<String> = self.map.keys().filter(|k| is_lod_key(k) && k.ends_with(&suffix)).cloned().collect();
        for k in lods {
            self.remove_lod(&k);
        }
    }

    /// Drop a rendering from the texture map (and the byte count); the LRU entry stays.
    fn drop_lod(&mut self, lod: &str) {
        if let Some(ImgState::Ready { size, .. }) = self.map.remove(lod) {
            self.lod_bytes = self.lod_bytes.saturating_sub(size[0] * size[1] * 4);
        }
    }

    fn remove_lod(&mut self, lod: &str) {
        self.drop_lod(lod);
        if let Some(i) = self.lru.iter().position(|k| k == lod) {
            self.lru.remove(i);
        }
    }

    /// The texture to draw an SVG image with when it covers `need_px` device pixels on its longer
    /// side: a rendering at least that large, started on a worker if there is none yet. Meanwhile
    /// (and for raster images, or when the base texture is already big enough) `None`: draw the
    /// base texture. Call every frame the image is drawn; renderings not asked for are dropped.
    pub fn lod_texture(&mut self, ctx: &egui::Context, cache_key: &str, need_px: f32) -> Option<TextureHandle> {
        let vector = self.svg.get(cache_key)?.clone();
        let want = lod_side(need_px, ctx.input(|i| i.max_texture_side))?;
        let wk = lod_key(cache_key, want);
        match self.map.get(&wk) {
            Some(ImgState::Ready { tex, .. }) => {
                let tex = tex.clone();
                self.touch(&wk);
                return Some(tex);
            }
            Some(ImgState::Loading) => self.want(&wk),
            Some(ImgState::Failed(_)) => {}
            None => self.queue(wk, Work::Render(vector, want)),
        }
        // Until it is ready, the sharpest rendering there is: a larger one first (zooming out),
        // else the largest smaller one (zooming in).
        let larger = std::iter::successors(Some(want * 2), |s| Some(s * 2)).take_while(|s| *s <= MAX_LOD_SIDE);
        let smaller = std::iter::successors(Some(want / 2), |s| Some(s / 2)).take_while(|s| *s >= MIN_LOD_SIDE);
        for side in larger.chain(smaller) {
            let k = lod_key(cache_key, side);
            if let Some(ImgState::Ready { tex, .. }) = self.map.get(&k) {
                let tex = tex.clone();
                self.touch(&k);
                return Some(tex);
            }
        }
        None
    }

    /// The texture to draw an SVG image with when it has `layers` restyled, and the cache key it
    /// is under (which serves [`lod_texture`](Self::lod_texture) as any SVG's does). The first
    /// time, the restyling is made on a worker; meanwhile the last one of this image there was,
    /// so dragging a layer's colour does not flash the original. `None` (draw the image as it
    /// is) for a picture that is not a loaded SVG, or with nothing restyled. Call every frame
    /// the image is drawn.
    pub fn restyled(&mut self, cache_key: &str, layers: &[LayerStyle]) -> Option<(String, TextureHandle)> {
        if layers.iter().all(LayerStyle::is_plain) {
            return None;
        }
        let vector = self.svg.get(cache_key)?.clone();
        let rk = restyle_key(cache_key, layers);
        match self.map.get(&rk) {
            Some(ImgState::Ready { tex, .. }) => {
                let tex = tex.clone();
                self.touch(&rk);
                self.last_restyled.insert(cache_key.to_string(), rk.clone());
                return Some((rk, tex));
            }
            Some(ImgState::Loading) => self.want(&rk),
            Some(ImgState::Failed(_)) => return None,
            None => self.queue(rk, Work::Restyle(vector, layers.to_vec())),
        }
        let last = self.last_restyled.get(cache_key)?.clone();
        match self.map.get(&last) {
            Some(ImgState::Ready { tex, .. }) => {
                let tex = tex.clone();
                self.touch(&last);
                Some((last, tex))
            }
            _ => None,
        }
    }

    fn queue(&mut self, key: String, work: Work) {
        self.map.insert(key.clone(), ImgState::Loading);
        self.loads_in_flight += 1;
        let job = Job { work, frame: self.frame, order: self.wanted.len() };
        self.wanted.push(key.clone());
        self.queue.jobs.lock().unwrap().insert(key, job);
        self.queue.ready.notify_one();
    }

    /// Web build: start the most wanted queued loads, a few at a time, so what is on screen
    /// loads first and stale requests are dropped from the queue as on the desktop.
    #[cfg(target_arch = "wasm32")]
    fn start_web_loads(&mut self) {
        // Renders run on this thread (there is no other), so at most one per frame.
        let mut rendered = false;
        while self.fetching < WEB_FETCHES {
            let Some((key, work)) = Queue::best(&mut self.queue.jobs.lock().unwrap()) else { break };
            match work {
                Work::Fetch(req) => {
                    self.fetching += 1;
                    self.load_web(key, req);
                }
                work => {
                    if rendered {
                        let job = Job { work, frame: self.frame, order: 0 };
                        self.queue.jobs.lock().unwrap().insert(key, job);
                        break;
                    }
                    rendered = true;
                    let _ = self.dtx.send((key, run(&work).map(|(l, _)| l)));
                }
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn load_web(&self, key: String, req: Request) {
        let dtx = self.dtx.clone();
        fetch_decode_web(req, Box::new(move |res| {
            let _ = dtx.send((key, res));
        }));
    }

    /// Note that a still-loading image was asked for again this frame.
    fn want(&mut self, key: &str) {
        if matches!(self.map.get(key), Some(ImgState::Loading)) {
            self.wanted.push(key.to_string());
        }
    }

    /// Re-rank queued loads by this frame's request order and drop the ones no longer asked for.
    fn reprioritize(&mut self) {
        let mut jobs = self.queue.jobs.lock().unwrap();
        for (i, key) in self.wanted.drain(..).enumerate() {
            if let Some(j) = jobs.get_mut(&key) {
                if j.frame < self.frame {
                    j.frame = self.frame;
                    j.order = i;
                }
            }
        }
        let frame = self.frame;
        let stale: Vec<String> = jobs.iter().filter(|(_, j)| j.frame + STALE_FRAMES < frame).map(|(k, _)| k.clone()).collect();
        for k in stale {
            jobs.remove(&k);
            self.map.remove(&k);
            self.loads_in_flight = self.loads_in_flight.saturating_sub(1);
        }
        drop(jobs);
        self.frame += 1;
    }

    fn touch(&mut self, key: &str) {
        if let Some(i) = self.lru.iter().position(|k| k == key) {
            self.lru.remove(i);
        }
        self.lru.push_back(key.to_string());
    }

    /// Poll decoded images and create textures. Call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        self.reprioritize();
        let mut any = false;
        #[cfg(target_arch = "wasm32")]
        self.start_web_loads();
        while let Ok((key, res)) = self.rx.try_recv() {
            #[cfg(target_arch = "wasm32")]
            {
                if !is_lod_key(&key) && !is_restyle_key(&key) {
                    self.fetching = self.fetching.saturating_sub(1);
                }
            }
            log::debug!("loaded {key}: {}", res.as_ref().map(|l| format!("{}x{}", l.width, l.height)).unwrap_or_else(|e| e.clone()));
            self.loads_in_flight = self.loads_in_flight.saturating_sub(1);
            any = true;
            // Dropped as stale (or forgotten) while it was in flight: nobody wants it any more.
            if !matches!(self.map.get(&key), Some(ImgState::Loading)) {
                continue;
            }
            match res {
                Ok(Loaded { width: w, height: h, rgba, svg }) => {
                    let ci = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                    let tex = ctx.load_texture(&key, ci, TextureOptions::LINEAR);
                    if is_lod_key(&key) {
                        self.lod_bytes += (w * h * 4) as usize;
                    } else {
                        if let Some(img) = RgbaImage::from_raw(w, h, rgba) {
                            self.cpu.insert(key.clone(), Arc::new(img));
                        }
                        match svg {
                            Some(vector) => self.svg.insert(key.clone(), vector),
                            None => self.svg.remove(&key),
                        };
                    }
                    self.map.insert(key, ImgState::Ready { tex, size: [w as usize, h as usize] });
                }
                Err(e) => {
                    log::warn!("image load failed: {e}");
                    self.map.insert(key, ImgState::Failed(e));
                }
            }
        }
        // Evict the least recently used textures (keep CPU copies only for live ones).
        while self.lru.len() > MAX_TEXTURES {
            if let Some(old) = self.lru.pop_front() {
                if matches!(self.map.get(&old), Some(ImgState::Ready { .. })) {
                    if is_lod_key(&old) {
                        self.drop_lod(&old);
                    } else {
                        self.map.remove(&old);
                        self.cpu.remove(&old);
                        self.svg.remove(&old);
                    }
                }
            }
        }
        // And the least recently drawn SVG renderings over the budget.
        while self.lod_bytes > LOD_BUDGET_BYTES {
            let Some(old) = self.lru.iter().find(|k| is_lod_key(k) && matches!(self.map.get(*k), Some(ImgState::Ready { .. }))).cloned() else { break };
            self.remove_lod(&old);
        }
        any
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="red"/></svg>"#;

    /// Below the base texture's size nothing extra is rendered; above it, the next power of two
    /// from 1024, capped by the GPU and our own limit.
    #[test]
    fn lod_sizes() {
        assert_eq!(lod_side(100.0, 8192), None);
        assert_eq!(lod_side(768.0, 8192), None);
        assert_eq!(lod_side(769.0, 8192), Some(1024));
        assert_eq!(lod_side(1024.0, 8192), Some(1024));
        assert_eq!(lod_side(1025.0, 8192), Some(2048));
        assert_eq!(lod_side(3000.0, 8192), Some(4096));
        assert_eq!(lod_side(9000.0, 8192), Some(4096));
        assert_eq!(lod_side(3000.0, 2048), Some(2048));
        assert_eq!(lod_side(3000.0, 512), None);
        assert_eq!(lod_side(f32::NAN, 8192), None);
    }

    /// An SVG decodes to the base pixels and keeps its vector form, which renders at any size
    /// with the drawing's aspect ratio; a PNG keeps none.
    #[test]
    fn svg_keeps_vector_form() {
        let l = decode(SVG).unwrap();
        assert_eq!((l.width, l.height), (MAX_SIDE, MAX_SIDE / 2));
        let vector = l.svg.expect("svg vector form");
        let big = vector.render(2048).unwrap();
        assert_eq!(big.dimensions(), (2048, 1024));
        assert_eq!(big.get_pixel(1000, 500).0, [255, 0, 0, 255]);
        assert!(matches!(run(&Work::Render(vector, 1024)), Ok((Loaded { width: 1024, height: 512, svg: None, .. }, None))));

        let mut png = Vec::new();
        RgbaImage::from_pixel(4, 2, image::Rgba([0, 0, 255, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let l = decode(&png).unwrap();
        assert_eq!((l.width, l.height), (4, 2));
        assert!(l.svg.is_none());
    }

    /// A file is cached by path; once forgotten (as uploading it again does) it is read afresh,
    /// so an edited file shows its new contents.
    #[test]
    fn forgotten_file_reloads() {
        let dir = std::env::temp_dir().join(format!("biocartoon-reload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cheese-bowl.svg");
        let p = path.display().to_string();
        let ctx = egui::Context::default();
        let mut cache = ImageCache::new(String::new(), None);
        let load = |cache: &mut ImageCache| {
            for _ in 0..500 {
                if let ImgState::Ready { size, .. } = cache.file(&p) {
                    return *size;
                }
                cache.poll(&ctx);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("{p} did not load");
        };
        let svg = |w, h| format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}"><rect width="{w}" height="{h}" fill="red"/></svg>"#);
        std::fs::write(&path, svg(40, 20)).unwrap();
        let first = load(&mut cache);
        std::fs::write(&path, svg(20, 40)).unwrap();
        assert_eq!(load(&mut cache), first, "cached until forgotten");
        cache.forget(&format!("file:{p}"));
        let second = load(&mut cache);
        assert!(first[0] > first[1] && second[0] < second[1], "{first:?} then {second:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An SVG whose root carries a namespace prefix (as Python's ElementTree writes NIAID
    /// BioArt files, e.g. Low Friction Flask) is still an SVG; text that merely mentions one
    /// is not.
    #[test]
    fn prefixed_svg_root() {
        let prefixed = br#"<?xml version='1.0' encoding='utf-8'?>
<ns0:svg xmlns:ns0="http://www.w3.org/2000/svg" viewBox="0 0 20 10"><metadata><title>Low Friction Flask</title></metadata><ns0:rect width="20" height="10" fill="red"/></ns0:svg>"#;
        assert!(is_svg(prefixed));
        assert!(is_svg(SVG));
        assert!(is_svg(b"<svg:svg xmlns:svg=\"http://www.w3.org/2000/svg\"/>"));
        assert!(!is_svg(b"<svgfoo/>"));
        assert!(!is_svg(b"<html>svg</html>"));
        // A PNG whose C2PA metadata embeds an SVG is still a PNG.
        let mut png = crate::ICON_PNG[..33].to_vec();
        png.extend_from_slice(b"\0\0\0\x40caBXjumb image/svg+xml <svg xmlns=\"http://www.w3.org/2000/svg\"/>");
        png.extend_from_slice(&crate::ICON_PNG[33..]);
        assert!(!is_svg(&png));
        let l = decode(prefixed).unwrap();
        assert_eq!((l.width, l.height), (MAX_SIDE, MAX_SIDE / 2));
        assert!(l.svg.is_some());
    }

    /// A catalog image whose SVG fetches but will not decode falls back to its PNG, so the
    /// canvas shows the picture rather than a "not loaded" box.
    #[test]
    fn undecodable_svg_falls_back_to_png() {
        let dir = std::env::temp_dir().join(format!("biocartoon-fallback-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("a.svg");
        std::fs::write(&bad, b"<svg xmlns=\"http://www.w3.org/2000/svg\"><rect").unwrap();
        let mut png = Vec::new();
        RgbaImage::from_pixel(4, 2, image::Rgba([0, 0, 255, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let good = dir.join("a.png");
        std::fs::write(&good, &png).unwrap();

        let req = Request::Either(Box::new(Request::Path(bad.clone())), Box::new(Request::Path(good.clone())));
        let (l, kept) = run(&Work::Fetch(req)).unwrap();
        assert_eq!((l.width, l.height), (4, 2));
        assert!(l.svg.is_none());
        assert!(kept.is_none(), "a fallback is not cached under the SVG's key");

        let missing = dir.join("none.png");
        let req = Request::Either(Box::new(Request::Path(bad)), Box::new(Request::Path(missing)));
        let e = run(&Work::Fetch(req)).err().expect("both choices fail");
        assert!(e.contains("none.png"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lod_keys() {
        let k = lod_key("asset:bioart/a.svg", 2048);
        assert_eq!(k, "lod:2048:asset:bioart/a.svg");
        assert!(is_lod_key(&k));
        assert!(!is_lod_key("asset:bioart/a.svg"));
        assert!(!is_lod_key("file:/x/lod:1.png"));
    }
}
