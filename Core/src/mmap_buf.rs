use std::fs::File;
use std::io;
use std::ops::Deref;
use std::path::Path;

/// Zero-copy read-only byte buffer backed either by POSIX `mmap` (zero dirty RSS on
/// macOS & iOS Keyboard Extensions) or an owned `Vec<u8>` (for in-memory tests).
pub enum ByteSource {
    #[cfg(unix)]
    Mmap {
        ptr: *mut core::ffi::c_void,
        len: usize,
    },
    Owned(Vec<u8>),
}

#[cfg(unix)]
unsafe impl Send for ByteSource {}
#[cfg(unix)]
unsafe impl Sync for ByteSource {}

#[cfg(unix)]
mod sys {
    use core::ffi::c_void;
    pub const PROT_READ: i32 = 0x01;
    pub const MAP_PRIVATE: i32 = 0x0002;

    extern "C" {
        pub fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: i32,
            flags: i32,
            fd: i32,
            offset: i64,
        ) -> *mut c_void;
        pub fn munmap(addr: *mut c_void, len: usize) -> i32;
    }
}

impl ByteSource {
    pub fn mmap_file<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let file = File::open(path.as_ref())?;
        let len = file.metadata()?.len() as usize;
        if len == 0 {
            return Ok(Self::Owned(Vec::new()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let ptr = unsafe {
                sys::mmap(
                    core::ptr::null_mut(),
                    len,
                    sys::PROT_READ,
                    sys::MAP_PRIVATE,
                    file.as_raw_fd(),
                    0,
                )
            };
            let failed = !0usize as *mut core::ffi::c_void;
            if ptr != failed && !ptr.is_null() {
                return Ok(Self::Mmap { ptr, len });
            }
        }
        // Fallback if mmap is unavailable
        let bytes = std::fs::read(path.as_ref())?;
        Ok(Self::Owned(bytes))
    }

    pub fn from_vec(bytes: Vec<u8>) -> Self {
        Self::Owned(bytes)
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        match self {
            #[cfg(unix)]
            Self::Mmap { ptr, len } => unsafe {
                core::slice::from_raw_parts(*ptr as *const u8, *len)
            },
            Self::Owned(v) => v.as_slice(),
        }
    }
}

impl Deref for ByteSource {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Drop for ByteSource {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Self::Mmap { ptr, len } = *self {
            if !ptr.is_null() && len > 0 {
                unsafe {
                    let _ = sys::munmap(ptr, len);
                }
            }
        }
    }
}

#[inline]
pub fn read_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

#[inline]
pub fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[inline]
pub fn read_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off],
        b[off + 1],
        b[off + 2],
        b[off + 3],
        b[off + 4],
        b[off + 5],
        b[off + 6],
        b[off + 7],
    ])
}

#[inline]
pub fn read_f32(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
