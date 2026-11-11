//! macOS: the icons Finder shows for files and folders, from `NSWorkspace`, as PNGs for GPUI.
//! Looked up by file type (extension), not by file, so a type is asked for once and no file is read.

// objc 0.2's macros test `cfg(feature = "cargo-clippy")`, which newer compilers flag.
#![allow(unexpected_cfgs)]

use std::collections::HashMap;
use std::ffi::{CString, c_void};
use std::sync::Arc;

use gpui::{Image, ImageFormat};
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};
use parking_lot::Mutex;

type Id = *mut Object;

#[repr(C)]
#[derive(Clone, Copy)]
struct NSPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NSSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NSRect {
    origin: NSPoint,
    size: NSSize,
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    fn NSFileTypeForHFSTypeCode(code: u32) -> Id;
}

/// `kGenericFolderIcon` and `kGenericDocumentIcon` as HFS type codes.
const FOLDER: u32 = u32::from_be_bytes(*b"fldr");
const DOCUMENT: u32 = u32::from_be_bytes(*b"docu");
/// `NSBitmapImageFileTypePNG`.
const PNG: usize = 4;

/// What to show: a folder, or a file with this (lowercase) extension ("" for none).
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    Folder,
    File(String),
}

impl Kind {
    pub fn of(name: &str, is_dir: bool) -> Self {
        if is_dir {
            return Kind::Folder;
        }
        let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("");
        Kind::File(ext.to_ascii_lowercase())
    }
}

/// Rendered icons by (kind, pixel size); `None` remembers that macOS had none.
type Cache = HashMap<(Kind, u32), Option<Arc<Image>>>;

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// Finder's icon for `kind`, rendered `pixels` wide (pass the size times the display scale).
/// `None` when macOS has no icon for it; callers fall back to the built-in icon.
pub fn icon(kind: &Kind, pixels: u32) -> Option<Arc<Image>> {
    let key = (kind.clone(), pixels);
    if let Some(hit) = CACHE.lock().get_or_insert_with(HashMap::new).get(&key) {
        return hit.clone();
    }
    // SAFETY: plain AppKit calls on the main thread (GPUI renders there), with objects we either
    // don't own (autoreleased) or release ourselves.
    let image = unsafe { render(kind, pixels) }.map(|png| Arc::new(Image::from_bytes(ImageFormat::Png, png)));
    CACHE.lock().get_or_insert_with(HashMap::new).insert(key, image.clone());
    image
}

unsafe fn render(kind: &Kind, pixels: u32) -> Option<Vec<u8>> {
    unsafe {
        let workspace: Id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let file_type: Id = match kind {
            Kind::Folder => NSFileTypeForHFSTypeCode(FOLDER),
            Kind::File(ext) if ext.is_empty() => NSFileTypeForHFSTypeCode(DOCUMENT),
            Kind::File(ext) => {
                let c = CString::new(ext.as_str()).ok()?;
                msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()]
            }
        };
        if workspace.is_null() || file_type.is_null() {
            return None;
        }
        let image: Id = msg_send![workspace, iconForFileType: file_type];
        if image.is_null() {
            return None;
        }
        // The proposed rect picks the best-fitting representation (16, 32, 128 … px).
        let side = f64::from(pixels.max(1));
        let mut rect = NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { width: side, height: side } };
        let nil: Id = std::ptr::null_mut();
        let cg_image: *mut c_void = msg_send![image, CGImageForProposedRect: &mut rect context: nil hints: nil];
        if cg_image.is_null() {
            return None;
        }
        let rep: Id = msg_send![class!(NSBitmapImageRep), alloc];
        let rep: Id = msg_send![rep, initWithCGImage: cg_image];
        if rep.is_null() {
            return None;
        }
        let properties: Id = msg_send![class!(NSDictionary), dictionary];
        let data: Id = msg_send![rep, representationUsingType: PNG properties: properties];
        let png = if data.is_null() {
            None
        } else {
            let length: usize = msg_send![data, length];
            let bytes: *const u8 = msg_send![data, bytes];
            (!bytes.is_null() && length > 0).then(|| std::slice::from_raw_parts(bytes, length).to_vec())
        };
        let _: () = msg_send![rep, release];
        png
    }
}
