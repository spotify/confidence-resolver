//! Convert a full-account JSON fixture to protobuf for local container tests.
use prost::Message;
use std::io::Write;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("Usage: encode_state <state.json>")?;
    let state: confidence_resolver::proto::confidence::flags::admin::v1::ResolverState =
        serde_json::from_reader(std::fs::File::open(path)?)?;
    std::io::stdout().write_all(&state.encode_to_vec())?;
    Ok(())
}
