//! Disk-backed CPU shadow storage for buffers that already live in VRAM.

use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READWRITE,
    UnmapViewOfFile,
};

pub(crate) struct Backing {
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    _file: File,
}

impl Backing {
    pub(crate) fn new(directory: &Path, size: usize) -> io::Result<Self> {
        let file = victory_core::sparse_file::create(directory, size as u64)?;
        // SAFETY: a new unnamed section of the file, with matching size.
        let section = unsafe {
            CreateFileMappingW(
                file.as_raw_handle(),
                std::ptr::null(),
                PAGE_READWRITE,
                (size as u64 >> 32) as u32,
                size as u32,
                std::ptr::null(),
            )
        };
        if section.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the unique new handle. The view retains the section.
        let section = unsafe { OwnedHandle::from_raw_handle(section) };
        // SAFETY: the backing file covers the entire requested view.
        let view = unsafe { MapViewOfFile(section.as_raw_handle(), FILE_MAP_WRITE, 0, 0, size) };
        if view.Value.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { view, _file: file })
    }

    pub(crate) fn data(&self) -> *mut u8 {
        self.view.Value.cast()
    }
}

impl Drop for Backing {
    fn drop(&mut self) {
        // SAFETY: our view outlives the CPU users, and the GPU sees only
        // the separate upload buffer.
        unsafe { UnmapViewOfFile(self.view) };
    }
}

/// File-backed staging is used with the synchronous direct-queue path.
pub(crate) fn directory() -> Option<&'static Path> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        let directory = std::env::var_os("VICTORY_CARD_STAGING_DIR")?;
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
        // SAFETY: symbol lookups in the process's ntdll.
        let wine = unsafe {
            let module = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
            !module.is_null()
                && GetProcAddress(module, c"wine_get_version".as_ptr().cast()).is_some()
        };
        if wine {
            tracing::warn!("file-backed card staging is available on native Windows");
            return None;
        }
        let copy_queues_requested = std::env::var("VICTORY_QUEUE2").ok()
            .and_then(|value| value.trim().parse::<u32>().ok()).unwrap_or(0) & 3 != 0;
        if crate::host::late() || crate::host::relay() || copy_queues_requested {
            tracing::warn!(
                "file-backed card staging requires synchronous direct-queue submissions"
            );
            return None;
        }
        Some(PathBuf::from(directory))
    })
    .as_deref()
}
