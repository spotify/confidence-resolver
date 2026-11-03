fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .bytes(["."])
        .compile_protos(&["proto/message.proto"], &["proto"])?;
    Ok(())
}
