use crate::{protocol::FileSpec, transfer::validate_name};
use anyhow::{Context, Result, bail, ensure};
use std::{fs::File, io::Cursor, path::PathBuf};
use windows::Win32::{
    Foundation::HANDLE,
    System::{DataExchange::*, Memory::*},
    UI::Shell::{DragQueryFileW, HDROP},
};

pub enum Source {
    File {
        file: File,
        path: PathBuf,
        modified: Option<std::time::SystemTime>,
    },
    Bytes(Cursor<Vec<u8>>),
}
pub struct Item {
    pub spec: FileSpec,
    pub source: Source,
}
pub fn has_rich_content() -> bool {
    unsafe {
        [15, 8, 17, 2]
            .into_iter()
            .any(|f| IsClipboardFormatAvailable(f).is_ok())
            || IsClipboardFormatAvailable(RegisterClipboardFormatW(windows::core::w!("PNG")))
                .is_ok()
    }
}
struct Open;
impl Open {
    fn new() -> Result<Self> {
        for _ in 0..10 {
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Ok(Self);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        bail!("clipboard is busy; try paste again")
    }
}
impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}
pub fn snapshot() -> Result<Vec<Item>> {
    let sequence = unsafe { GetClipboardSequenceNumber() };
    let opened = Open::new()?;
    if unsafe { IsClipboardFormatAvailable(15) }.is_ok() {
        let handle = unsafe { GetClipboardData(15) }?;
        let hdrop = HDROP(handle.0);
        let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
        ensure!(
            count > 0 && count as usize <= crate::protocol::MAX_FILES,
            "unsupported file count"
        );
        let mut paths = Vec::new();
        for index in 0..count {
            let len = unsafe { DragQueryFileW(hdrop, index, None) };
            let mut text = vec![0; len as usize + 1];
            unsafe {
                DragQueryFileW(hdrop, index, Some(&mut text));
            }
            paths.push(PathBuf::from(String::from_utf16(&text[..len as usize])?));
        }
        drop(opened);
        let mut items = Vec::new();
        for path in paths {
            let metadata = std::fs::symlink_metadata(&path)?;
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "folders and links are not supported: {}",
                path.display()
            );
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .context("filename is not valid Unicode")?
                .to_string();
            validate_name(&name)?;
            use std::os::windows::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&path)?;
            let metadata = file.metadata()?;
            items.push(Item {
                spec: FileSpec {
                    name,
                    size: metadata.len(),
                },
                source: Source::File {
                    file,
                    path,
                    modified: metadata.modified().ok(),
                },
            });
        }
        return Ok(items);
    }
    let png_format = unsafe { RegisterClipboardFormatW(windows::core::w!("PNG")) };
    if unsafe { IsClipboardFormatAvailable(png_format) }.is_ok() {
        let data = copy_global(unsafe { GetClipboardData(png_format) }?)?;
        drop(opened);
        // Verify PNG structure before transferring a purported clipboard image.
        image::load_from_memory_with_format(&data, image::ImageFormat::Png)?;
        return Ok(vec![image_item(data)]);
    }
    drop(opened);
    let mut clipboard = arboard::Clipboard::new()?;
    let image = clipboard
        .get_image()
        .context("clipboard format is unsupported (virtual attachments are not files)")?;
    ensure!(
        unsafe { GetClipboardSequenceNumber() } == sequence,
        "clipboard changed during paste; try again"
    );
    let pixels = image::RgbaImage::from_raw(
        image.width.try_into()?,
        image.height.try_into()?,
        image.bytes.into_owned(),
    )
    .context("invalid image dimensions")?;
    let mut encoded = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels).write_to(&mut encoded, image::ImageFormat::Png)?;
    Ok(vec![image_item(encoded.into_inner())])
}
fn image_item(data: Vec<u8>) -> Item {
    Item {
        spec: FileSpec {
            name: "clipboard.png".into(),
            size: data.len() as u64,
        },
        source: Source::Bytes(Cursor::new(data)),
    }
}
fn copy_global(handle: HANDLE) -> Result<Vec<u8>> {
    let global = windows::Win32::Foundation::HGLOBAL(handle.0);
    let size = unsafe { GlobalSize(global) };
    ensure!(
        size > 0 && size <= 256 * 1024 * 1024,
        "clipboard image is empty or exceeds 256 MiB"
    );
    let ptr = unsafe { GlobalLock(global) };
    ensure!(!ptr.is_null(), "cannot read clipboard image");
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), size) }.to_vec();
    unsafe {
        let _ = GlobalUnlock(global);
    }
    Ok(bytes)
}
