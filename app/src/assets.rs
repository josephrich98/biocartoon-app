//! Image loading: fetches PNGs from the R2 bucket (or local files), decodes them on worker
//! threads and turns them into egui textures on the UI thread. LRU-evicts GPU textures.
//!
//! Pending loads are a priority queue rather than a FIFO: each frame, images are ranked in the
//! order the UI asked for them, so a grid fills in reading order and images that are no longer
//! drawn (e.g. after switching tabs) are dropped instead of delaying the visible ones.

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
const MAX_SIDE: u32 = 768;
const MAX_TEXTURES: usize = 400;

#[derive(Clone, Debug)]
pub enum Request {
    /// Fetch `<base_url>/<key>`.
    Url(String),
    /// Read from disk.
    Path(PathBuf),
}

pub enum ImgState {
    Loading,
    Ready { tex: TextureHandle, size: [usize; 2] },
    Failed(String),
}

struct Job {
    req: Request,
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
    fn best(jobs: &mut HashMap<String, Job>) -> Option<(String, Request)> {
        let key = jobs.iter().max_by_key(|(_, j)| (j.frame, std::cmp::Reverse(j.order))).map(|(k, _)| k.clone())?;
        let job = jobs.remove(&key)?;
        Some((key, job.req))
    }

    /// Block until there is a job.
    #[cfg(not(target_arch = "wasm32"))]
    fn take(&self) -> (String, Request) {
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

type Decoded = (String, Result<(u32, u32, Vec<u8>), String>);

pub struct ImageCache {
    map: HashMap<String, ImgState>,
    /// Decoded CPU copies, kept for the AI edit operations and export.
    cpu: HashMap<String, Arc<RgbaImage>>,
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
    match req {
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

pub fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let img = img.to_rgba8();
    let (w, h) = img.dimensions();
    let m = w.max(h);
    if m > MAX_SIDE {
        let s = MAX_SIDE as f32 / m as f32;
        let nw = ((w as f32 * s) as u32).max(1);
        let nh = ((h as f32 * s) as u32).max(1);
        Ok(image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle))
    } else {
        Ok(img)
    }
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
                    let (key, req) = queue.take();
                    log::debug!("loading {req:?}");
                    let res = fetch(&req)
                        .and_then(|b| {
                            let img = decode(&b)?;
                            if let (Request::Url(_), Some(dir), Some(k)) = (&req, cache_dir.read().unwrap().clone(), key.strip_prefix("asset:")) {
                                crate::offline::store(&dir, k, &b);
                            }
                            Ok(img)
                        })
                        .map(|img| {
                            let (w, h) = img.dimensions();
                            (w, h, img.into_raw())
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
                // (wasm32's `is_absolute` is false even for "/..."; app data there lives under "/biocartoon".)
                _ if std::path::Path::new(rel).is_absolute() || (cfg!(target_arch = "wasm32") && rel.starts_with('/')) => Some(std::path::PathBuf::from(rel)),
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

    /// Where to load the full-resolution original of a source (used by export): the offline
    /// copy when there is one, else the asset URL.
    pub fn request_for(&self, src: &crate::doc::ImageSource) -> Result<Request, String> {
        match src {
            crate::doc::ImageSource::Asset { key, .. } => self.request_for_asset(key, None),
            crate::doc::ImageSource::File { path, .. } => Ok(Request::Path(PathBuf::from(path))),
        }
    }

    /// Ask for an asset PNG by R2 key. Returns the current state.
    pub fn asset(&mut self, key: &str, local_rel: Option<&str>) -> &ImgState {
        let ck = format!("asset:{key}");
        if !self.map.contains_key(&ck) {
            match self.request_for_asset(key, local_rel) {
                Ok(req) => self.queue(ck.clone(), req),
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
            self.queue(ck.clone(), Request::Path(PathBuf::from(path)));
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

    /// Replace the cached texture for a key with a new image (after an AI edit) and return it.
    pub fn insert_image(&mut self, ctx: &egui::Context, cache_key: &str, img: RgbaImage) {
        let (w, h) = img.dimensions();
        let ci = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
        let tex = ctx.load_texture(cache_key, ci, TextureOptions::LINEAR);
        self.cpu.insert(cache_key.to_string(), Arc::new(img));
        self.map.insert(cache_key.to_string(), ImgState::Ready { tex, size: [w as usize, h as usize] });
        self.touch(cache_key);
    }

    pub fn forget(&mut self, cache_key: &str) {
        self.map.remove(cache_key);
        self.cpu.remove(cache_key);
    }

    fn queue(&mut self, key: String, req: Request) {
        self.map.insert(key.clone(), ImgState::Loading);
        self.loads_in_flight += 1;
        let job = Job { req, frame: self.frame, order: self.wanted.len() };
        self.wanted.push(key.clone());
        self.queue.jobs.lock().unwrap().insert(key, job);
        self.queue.ready.notify_one();
    }

    /// Web build: start the most wanted queued loads, a few at a time, so what is on screen
    /// loads first and stale requests are dropped from the queue as on the desktop.
    #[cfg(target_arch = "wasm32")]
    fn start_web_loads(&mut self) {
        while self.fetching < WEB_FETCHES {
            let Some((key, req)) = Queue::best(&mut self.queue.jobs.lock().unwrap()) else { break };
            self.fetching += 1;
            self.load_web(key, req);
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn load_web(&self, key: String, req: Request) {
        fn finish(bytes: Result<Vec<u8>, String>) -> Result<(u32, u32, Vec<u8>), String> {
            let img = decode(&bytes?)?;
            let (w, h) = img.dimensions();
            Ok((w, h, img.into_raw()))
        }
        let dtx = self.dtx.clone();
        match req {
            Request::Path(_) => {
                let _ = dtx.send((key, finish(fetch(&req))));
            }
            Request::Url(url) => ehttp::fetch(ehttp::Request::get(&url), move |res| {
                let bytes = res.map_err(|e| format!("GET {url}: {e}")).and_then(|r| {
                    if r.ok { Ok(r.bytes) } else { Err(format!("GET {url}: {} {}", r.status, r.status_text)) }
                });
                let _ = dtx.send((key, finish(bytes)));
            }),
        }
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
                self.fetching = self.fetching.saturating_sub(1);
            }
            log::debug!("loaded {key}: {}", res.as_ref().map(|r| format!("{}x{}", r.0, r.1)).unwrap_or_else(|e| e.clone()));
            self.loads_in_flight = self.loads_in_flight.saturating_sub(1);
            any = true;
            match res {
                Ok((w, h, raw)) => {
                    let ci = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &raw);
                    let tex = ctx.load_texture(&key, ci, TextureOptions::LINEAR);
                    if let Some(img) = RgbaImage::from_raw(w, h, raw) {
                        self.cpu.insert(key.clone(), Arc::new(img));
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
                    self.map.remove(&old);
                    self.cpu.remove(&old);
                }
            }
        }
        any
    }
}
