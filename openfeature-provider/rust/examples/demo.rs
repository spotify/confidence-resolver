//! Demo application for the Confidence OpenFeature provider.
//!
//! This example shows how to use the native Rust provider with the OpenFeature SDK.
//!
//! ## Running
//!
//! ```bash
//! cargo run --example demo
//! ```

use open_feature::{EvaluationContext, OpenFeature, StructValue};
use spotify_confidence_openfeature_provider::{ConfidenceProvider, ProviderOptions};

// Configuration - replace with your actual values
const CLIENT_SECRET: &str = "CLIENT_SECRET";
const FLAG_KEY: &str = "hawkflag";
const TARGETING_KEY: &str = "dennis";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing for logging
    tracing_subscriber::fmt::init();

    println!("=== Confidence OpenFeature Provider Demo ===");
    println!();

    // Create provider options
    let options = ProviderOptions::new(CLIENT_SECRET);

    // Create the Confidence provider
    println!("Creating Confidence provider...");
    let provider = ConfidenceProvider::new(options)?;

    // Set the provider on the OpenFeature singleton
    println!("Setting provider on OpenFeature...");
    OpenFeature::singleton_mut().await.set_provider(provider).await;

    println!("Provider initialized successfully!");
    println!();

    // Create an OpenFeature client
    let client = OpenFeature::singleton().await.create_client();

    // Create evaluation context with targeting key
    let context = EvaluationContext::default()
        .with_targeting_key(TARGETING_KEY)
        .with_custom_field("environment", "production");

    println!("Evaluating flags for visitor_id: {}", TARGETING_KEY);
    println!();

    // Evaluate boolean flag using OpenFeature API
    println!("--- Boolean: {}.enabled ---", FLAG_KEY);
    let bool_result = client
        .get_bool_details(&format!("{}.enabled", FLAG_KEY), Some(&context), None)
        .await;
    match bool_result {
        Ok(details) => {
            println!("  Value: {}", details.value);
            println!("  Variant: {:?}", details.variant);
            println!("  Reason: {:?}", details.reason);
        }
        Err(e) => println!("  Error: {:?}", e),
    }
    println!();

    // Evaluate string flag using OpenFeature API
    println!("--- String: {}.message ---", FLAG_KEY);
    let string_result = client
        .get_string_details(&format!("{}.message", FLAG_KEY), Some(&context), None)
        .await;
    match string_result {
        Ok(details) => {
            println!("  Value: {}", details.value);
            println!("  Variant: {:?}", details.variant);
            println!("  Reason: {:?}", details.reason);
        }
        Err(e) => println!("  Error: {:?}", e),
    }
    println!();

    // Evaluate another string field
    println!("--- String: {}.color ---", FLAG_KEY);
    let color_result = client
        .get_string_details(&format!("{}.color", FLAG_KEY), Some(&context), None)
        .await;
    match color_result {
        Ok(details) => {
            println!("  Value: {}", details.value);
            println!("  Variant: {:?}", details.variant);
            println!("  Reason: {:?}", details.reason);
        }
        Err(e) => println!("  Error: {:?}", e),
    }
    println!();

    // Evaluate struct (full flag value) using OpenFeature API
    println!("--- Struct: {} (full value) ---", FLAG_KEY);
    let struct_result = client
        .get_struct_details::<StructValue>(FLAG_KEY, Some(&context), None)
        .await;
    match struct_result {
        Ok(details) => {
            println!("  Value: {:?}", details.value);
            println!("  Variant: {:?}", details.variant);
            println!("  Reason: {:?}", details.reason);
        }
        Err(e) => println!("  Error: {:?}", e),
    }
    println!();

    println!("Done!");

    Ok(())
}
