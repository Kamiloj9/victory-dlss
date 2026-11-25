//! Temporary zero-filled backing for mapped title memory on Windows.

use std::fs::File;
use std::io;
use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
use std::path::Path;
use std::ptr;
use windows_sys::Win32::Foundation::ERROR_DISK_FULL;
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FILE_ZERO_DATA_INFORMATION, FSCTL_SET_SPARSE, FSCTL_SET_ZERO_DATA,
};

/// A temporary sparse file of `size` bytes, zeroed before it can be mapped.
/// The caller keeps the file open as long as it needs the backing.
pub fn create(directory: &Path, size: u64) -> io::Result<File> {
    crate::ensure_outside_repo(directory).map_err(io::Error::other)?;
    let path: Vec<u16> = directory.as_os_str().encode_wide().chain([0]).collect();
    let mut available = 0;
    // SAFETY: path is terminated and available is writable.
    if unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if available < size {
        return Err(io::Error::from_raw_os_error(ERROR_DISK_FULL as i32));
    }
    let file = tempfile::tempfile_in(directory)?;
    let end = i64::try_from(size).map_err(io::Error::other)?;
    let mut returned = 0;
    let mut control = |code, input, input_len| {
        // SAFETY: the live synchronous file and input parameters for the
        // specified filesystem control. Neither operation has output.
        unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                code,
                input,
                input_len,
                ptr::null_mut(),
                0,
                &mut returned,
                ptr::null_mut(),
            )
        }
    };
    if control(FSCTL_SET_SPARSE, ptr::null(), 0) == 0 {
        return Err(io::Error::last_os_error());
    }
    file.set_len(size)?;
    let zero = FILE_ZERO_DATA_INFORMATION {
        FileOffset: 0,
        BeyondFinalZero: end,
    };
    if control(
        FSCTL_SET_ZERO_DATA,
        (&raw const zero).cast(),
        size_of_val(&zero) as u32,
    ) == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}
