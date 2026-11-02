//! egui's SVG loader only recognizes URIs ending in `.svg`. Web images such as README badges
//! (`https://img.shields.io/badge/...`) are SVGs served without that extension, so this loader
//! detects SVG by content type / content instead.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use egui::load::{BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint};

type Cache = HashMap<(String, SizeHint), Result<Arc<egui::ColorImage>, String>>;

#[derive(Default)]
pub struct SvgByContentLoader {
    cache: Mutex<Cache>,
}

fn options() -> &'static resvg::usvg::Options<'static> {
    static OPTS: OnceLock<resvg::usvg::Options<'static>> = OnceLock::new();
    OPTS.get_or_init(|| {
        let mut o = resvg::usvg::Options::default();
        o.fontdb_mut().load_system_fonts();
        o
    })
}

fn looks_like_svg(bytes: &[u8], mime: Option<&str>) -> bool {
    if mime.is_some_and(|m| m.contains("svg")) {
        return true;
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    head.trim_start().starts_with("<svg") || (head.trim_start().starts_with("<?xml") && head.contains("<svg"))
}

impl ImageLoader for SvgByContentLoader {
    fn id(&self) -> &str {
        "vault::SvgByContentLoader"
    }

    fn load(&self, ctx: &egui::Context, uri: &str, size_hint: SizeHint) -> ImageLoadResult {
        // `.svg` URIs and embedded bytes are handled by egui_extras already.
        if uri.ends_with(".svg") || !(uri.starts_with("http://") || uri.starts_with("https://")) {
            return Err(LoadError::NotSupported);
        }
        let key = (uri.to_string(), size_hint);
        if let Some(r) = self.cache.lock().unwrap().get(&key) {
            return r.clone().map(|image| ImagePoll::Ready { image }).map_err(LoadError::Loading);
        }
        match ctx.try_load_bytes(uri) {
            Ok(BytesPoll::Ready { bytes, mime, .. }) => {
                if !looks_like_svg(&bytes, mime.as_deref()) {
                    return Err(LoadError::NotSupported);
                }
                let result = egui_extras::image::load_svg_bytes_with_size(&bytes, size_hint, options()).map(Arc::new);
                self.cache.lock().unwrap().insert(key, result.clone());
                result.map(|image| ImagePoll::Ready { image }).map_err(LoadError::Loading)
            }
            Ok(BytesPoll::Pending { size }) => Ok(ImagePoll::Pending { size }),
            Err(e) => Err(e),
        }
    }

    fn forget(&self, uri: &str) {
        self.cache.lock().unwrap().retain(|(u, _), _| u != uri);
    }

    fn forget_all(&self) {
        self.cache.lock().unwrap().clear();
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .unwrap()
            .values()
            .map(|r| r.as_ref().map_or(0, |img| img.pixels.len() * 4))
            .sum()
    }
}
