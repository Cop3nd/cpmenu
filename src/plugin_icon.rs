//! Icons that plugins name for their rows: png/ico/jpg files up to 256 KB, decoded with WIC into
//! the same 32x32 premultiplied BGRA the app icons use. Files only, never a URL. A small LRU keyed
//! by path and modification time keeps the repeated rows of a typing burst from decoding twice.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, IWICPalette, WICBitmapDitherTypeNone,
    WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::HSTRING;

use crate::apps::{ICON, Icon};

const MAX_BYTES: u64 = 256 * 1024;
const CACHE: usize = 64;

thread_local! {
    static LRU: RefCell<Vec<(PathBuf, SystemTime, Vec<u8>)>> = const { RefCell::new(Vec::new()) };
}

/// Only picture files of a size a row icon needs.
pub fn acceptable(path: &Path) -> bool {
    let known = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["png", "ico", "jpg", "jpeg"].iter().any(|k| e.eq_ignore_ascii_case(k)));
    known && std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES)
}

/// The icon in `path`, or `None` if it is not an acceptable, decodable picture.
pub fn load(path: &Path) -> Option<Icon> {
    if !acceptable(path) {
        return None;
    }
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let cached = LRU.with_borrow_mut(|lru| {
        let at = lru.iter().position(|(p, m, _)| p == path && *m == modified)?;
        let entry = lru.remove(at);
        let pixels = entry.2.clone();
        lru.insert(0, entry);
        Some(pixels)
    });
    if let Some(pixels) = cached {
        return Some(Icon { color: pixels });
    }
    let pixels = decode(path)?;
    LRU.with_borrow_mut(|lru| {
        lru.insert(0, (path.to_owned(), modified, pixels.clone()));
        lru.truncate(CACHE);
    });
    Some(Icon { color: pixels })
}

fn decode(path: &Path) -> Option<Vec<u8>> {
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let decoder = factory
            .CreateDecoderFromFilename(&HSTRING::from(path.as_os_str()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let converter = factory.CreateFormatConverter().ok()?;
        converter
            .Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None::<&IWICPalette>, 0.0, WICBitmapPaletteTypeCustom)
            .ok()?;
        let scaler = factory.CreateBitmapScaler().ok()?;
        scaler.Initialize(&converter, ICON as u32, ICON as u32, WICBitmapInterpolationModeFant).ok()?;
        let mut pixels = vec![0u8; ICON * ICON * 4];
        scaler.CopyPixels(std::ptr::null(), (ICON * 4) as u32, &mut pixels).ok()?;
        Some(pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_small_picture_files_are_accepted() {
        let dir = std::env::temp_dir().join(format!("cpmenu-test-icon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (ok, big, text) = (dir.join("a.PNG"), dir.join("b.png"), dir.join("c.txt"));
        std::fs::write(&ok, [0u8; 10]).unwrap();
        std::fs::write(&big, vec![0u8; MAX_BYTES as usize + 1]).unwrap();
        std::fs::write(&text, "x").unwrap();
        assert!(acceptable(&ok) && !acceptable(&big) && !acceptable(&text) && !acceptable(&dir.join("missing.png")));
        assert!(load(&ok).is_none(), "ten zero bytes are not a picture");
        let _ = std::fs::remove_dir_all(dir);
    }
}
