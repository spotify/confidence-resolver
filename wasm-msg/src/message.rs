use crate::memory::{consume_buffer, take_buffer, transfer_buffer};
use prost::Message;

// Include the generated protobuf code
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/wasm_msg.rs"));
}

/// Consumes a request from guest memory, decoding it and freeing the memory.
/// Returns the decoded request data.
pub(crate) fn consume_request<T>(ptr: *mut u8) -> T
where
    T: prost::Message + Default,
{
    // Both decodes slice the one buffer: `data` is a view into it, and a
    // `bytes` field of T decoded as `Bytes` is a view into `data`.
    let request = proto::Request::decode(take_buffer(ptr))
        .expect("consume_request: failed to decode request");
    T::decode(request.data).expect("consume_request: failed to decode request")
}

/// Consumes a response from host memory, decoding it and freeing the memory.
/// Returns the decoded response data or error.
pub(crate) fn consume_response<T>(ptr: *mut u8) -> Result<T, String>
where
    T: prost::Message + Default,
{
    // First consume the response wrapper
    let response = consume_message::<proto::Response>(ptr);

    // Extract the response from the wrapper
    match response.result {
        Some(proto::response::Result::Data(data)) => {
            let result = T::decode(data).expect("consume_response: failed to decode response");
            Ok(result)
        }
        Some(proto::response::Result::Error(e)) => Err(e),
        _ => panic!("consume_response: invalid response type"),
    }
}

/// Transfers a request to guest memory, encoding it and allocating memory.
/// Returns a pointer to the allocated memory containing the encoded request.
pub(crate) fn transfer_request<T>(request: T) -> *mut u8
where
    T: prost::Message,
{
    // First encode the request
    let mut encoded = Vec::new();
    request
        .encode(&mut encoded)
        .expect("transfer_request: failed to encode request");

    // Create and transfer the request wrapper
    let request = proto::Request {
        data: encoded.into(),
    };
    transfer_message(request)
}

/// Transfers a response to host memory, encoding it and allocating memory.
/// Returns a pointer to the allocated memory containing the encoded response.
pub(crate) fn transfer_response<T>(response: Result<T, String>) -> *mut u8
where
    T: prost::Message,
{
    // Create the response wrapper
    let response = match response {
        Ok(resp) => {
            // Encode the response
            let mut encoded = Vec::new();
            resp.encode(&mut encoded)
                .expect("transfer_response: failed to encode response");
            proto::Response {
                result: Some(proto::response::Result::Data(encoded.into())),
            }
        }
        Err(e) => proto::Response {
            result: Some(proto::response::Result::Error(e)),
        },
    };

    // Transfer the response wrapper
    transfer_message(response)
}

/// Consume a message from memory, decoding it and freeing the allocation.
/// The pointer should point to the data area (after the size field).
/// Returns the decoded message or an error.
pub(crate) fn consume_message<T>(ptr: *mut u8) -> T
where
    T: prost::Message + Default,
{
    if ptr.is_null() {
        panic!("consume_message: called with null pointer");
    }

    // Decode the message
    consume_buffer(ptr, |buf| {
        T::decode(buf).expect("consume_message: failed to decode message")
    })
}

/// Transfer a message to memory, encoding it and allocating memory.
/// Returns a pointer to the allocated memory containing the encoded message.
pub(crate) fn transfer_message<T>(message: T) -> *mut u8
where
    T: prost::Message,
{
    // Encode the message
    let mut encoded = Vec::new();
    message
        .encode(&mut encoded)
        .expect("transfer_message: failed to encode message");

    transfer_buffer(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::wasm_msg_alloc;

    /// Writes `message` into a wasm_msg buffer the way a host does.
    fn host_transfer<T: prost::Message>(message: &T) -> *mut u8 {
        let encoded = message.encode_to_vec();
        let ptr = wasm_msg_alloc(encoded.len());
        unsafe { core::ptr::copy_nonoverlapping(encoded.as_ptr(), ptr, encoded.len()) };
        ptr
    }

    fn payload() -> Vec<u8> {
        (0..100_000u32).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn consume_request_decodes_a_bytes_field_without_copying() {
        // A Request whose data is itself a Request: the inner `data` is the
        // `bytes` field the guest receives.
        let inner = proto::Request {
            data: payload().into(),
        };
        let ptr = host_transfer(&proto::Request {
            data: inner.encode_to_vec().into(),
        });
        let start = ptr as usize;
        let end = start
            + proto::Request {
                data: inner.encode_to_vec().into(),
            }
            .encoded_len();

        let decoded: proto::Request = consume_request(ptr);

        assert_eq!(decoded.data.as_ref(), payload().as_slice());
        let at = decoded.data.as_ptr() as usize;
        assert!(
            start <= at && at + decoded.data.len() <= end,
            "bytes field must view the host buffer"
        );
    }

    #[test]
    fn a_slice_of_the_request_outlives_the_decoded_message() {
        let inner = proto::Request {
            data: payload().into(),
        };
        let ptr = host_transfer(&proto::Request {
            data: inner.encode_to_vec().into(),
        });
        let decoded: proto::Request = consume_request(ptr);
        let tail = decoded.data.slice(99_000..);
        drop(decoded);
        assert_eq!(tail.as_ref(), &payload()[99_000..]);
    }

    #[test]
    fn request_and_response_round_trip() {
        let ptr = transfer_request(proto::Request {
            data: payload().into(),
        });
        let decoded: proto::Request = consume_request(ptr);
        assert_eq!(decoded.data.as_ref(), payload().as_slice());

        let ptr = transfer_response::<proto::Request>(Ok(proto::Request {
            data: payload().into(),
        }));
        let decoded: proto::Request = consume_response(ptr).unwrap();
        assert_eq!(decoded.data.as_ref(), payload().as_slice());

        let ptr = transfer_response::<proto::Request>(Err("boom".to_string()));
        assert_eq!(consume_response::<proto::Request>(ptr).unwrap_err(), "boom");
    }
}
