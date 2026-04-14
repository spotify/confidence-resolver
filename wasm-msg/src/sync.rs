use crate::message;

pub type WasmResult<T> = core::result::Result<T, String>;

pub fn call_sync_guest<F, Req, Res>(ptr: *mut u8, handler: F) -> *mut u8
where
    F: FnOnce(Req) -> WasmResult<Res>,
    Req: prost::Message + Default,
    Res: prost::Message,
{
    let request = if ptr.is_null() {
        Req::default()
    } else {
        message::consume_request::<Req>(ptr)
    };
    let result = handler(request);
    message::transfer_response(result)
}

pub fn call_sync_host<Req, Res>(
    request: Req,
    host_func: unsafe extern "C" fn(*mut u8) -> *mut u8,
) -> WasmResult<Res>
where
    Req: prost::Message,
    Res: prost::Message + Default,
{
    let input_ptr = message::transfer_request(request);
    let output_ptr = unsafe { host_func(input_ptr) };
    // Free the request we allocated — the host has already read it.
    // This mirrors call_sync_guest which frees its input via consume_request.
    crate::memory::wasm_msg_free(input_ptr);
    if output_ptr.is_null() {
        return Err(String::from("Host function returned null pointer"));
    }
    message::consume_response::<Res>(output_ptr)
}
