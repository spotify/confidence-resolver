use core::mem;
use core::ptr;
use std::alloc::{alloc, dealloc, Layout};

/// Custom allocation function that stores total allocation size before the data
/// Returns a pointer to the data area (after the size)
#[doc(hidden)]
#[no_mangle]
pub extern "C" fn wasm_msg_alloc(size: usize) -> *mut u8 {
    // Calculate total allocation size (including the size field itself)
    let total_size = size + mem::size_of::<usize>();
    let layout = unsafe { Layout::from_size_align_unchecked(total_size, mem::align_of::<usize>()) };

    let ptr = unsafe { alloc(layout) };
    if ptr.is_null() {
        return ptr::null_mut();
    }

    // Store the total allocation size at the start
    unsafe {
        ptr::write(ptr as *mut usize, total_size);
    }

    // Return pointer to the data area (after the size)
    unsafe { ptr.add(mem::size_of::<usize>()) }
}

/// Custom free function that reads total allocation size from before the data
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[doc(hidden)]
#[no_mangle]
pub extern "C" fn wasm_msg_free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }

    // Get pointer to the size (before the data)
    let size_ptr = unsafe { ptr.sub(mem::size_of::<usize>()) };

    // Read the total allocation size
    let total_size = unsafe { ptr::read(size_ptr as *const usize) };
    let layout = unsafe { Layout::from_size_align_unchecked(total_size, mem::align_of::<usize>()) };

    // Free the entire allocation
    unsafe {
        dealloc(size_ptr, layout);
    }
}

/// View the buffer at the given pointer, returning a slice to the data.
/// The pointer should point to the data area (after the size field).
/// This is unsafe because it assumes the pointer is valid and the size field is correct.
/// The returned slice is only valid as long as the memory at ptr is valid.
fn view_buffer<'a>(ptr: *mut u8) -> &'a [u8] {
    // Safety: We trust the caller to provide valid memory
    unsafe { core::slice::from_raw_parts(ptr, buffer_len(ptr)) }
}

pub(crate) fn consume_buffer<F, R>(ptr: *mut u8, f: F) -> R
where
    F: FnOnce(&[u8]) -> R,
{
    let buf = view_buffer(ptr);
    let result = f(buf);
    wasm_msg_free(ptr);
    result
}

pub(crate) fn transfer_buffer(buf: Vec<u8>) -> *mut u8 {
    let ptr = wasm_msg_alloc(buf.len());
    if ptr.is_null() {
        panic!("transfer_buffer: failed to allocate memory");
    }
    unsafe {
        ptr::copy_nonoverlapping(buf.as_ptr(), ptr, buf.len());
    }
    ptr
}

/// A wasm_msg buffer owned as `Bytes`: the allocation is freed when the last
/// slice of it drops, so a decoded `bytes` field can point into it instead of
/// copying, and the payload of a large message is held once rather than once
/// per decoding layer. A `Bytes` kept past the call keeps the whole buffer alive.
struct OwnedBuffer {
    ptr: *mut u8,
    len: usize,
}

// The buffer is a global-allocator allocation read only through this owner,
// so it can be freed from any thread, like a Box<[u8]>.
unsafe impl Send for OwnedBuffer {}

impl AsRef<[u8]> for OwnedBuffer {
    fn as_ref(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for OwnedBuffer {
    fn drop(&mut self) {
        wasm_msg_free(self.ptr);
    }
}

/// Takes ownership of the `wasm_msg_alloc` buffer at `ptr` as `Bytes`; see
/// [`OwnedBuffer`].
pub(crate) fn take_buffer(ptr: *mut u8) -> bytes::Bytes {
    assert!(!ptr.is_null(), "take_buffer: called with null pointer");
    let len = buffer_len(ptr);
    bytes::Bytes::from_owner(OwnedBuffer { ptr, len })
}

/// Data length of the buffer at `ptr`, read from the size prefix.
fn buffer_len(ptr: *mut u8) -> usize {
    let total_size = unsafe { ptr::read(ptr.sub(mem::size_of::<usize>()) as *const usize) };
    total_size
        .checked_sub(mem::size_of::<usize>())
        .expect("buffer_len: size prefix smaller than the prefix itself")
}
