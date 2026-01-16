//! E2E tests for the Confidence OpenFeature provider.
//!
//! These tests use real Confidence flags and the same test data as the JS provider tests.
//!
//! Run with: cargo test --test e2e

use open_feature::{EvaluationContext, EvaluationReason, OpenFeature, StructValue, Value};
use spotify_confidence_openfeature_provider::{ConfidenceProvider, ProviderOptions};

const CLIENT_SECRET: &str = "ti5Sipq5EluCYRG7I5cdbpWC3xq7JTWv";
const TARGETING_KEY: &str = "test-a"; // control variant

fn context() -> EvaluationContext {
    EvaluationContext::default()
        .with_targeting_key(TARGETING_KEY)
        .with_custom_field("sticky", false)
}

fn sticky_context() -> EvaluationContext {
    EvaluationContext::default()
        .with_targeting_key(TARGETING_KEY)
        .with_custom_field("sticky", true)
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_tests() {
    // Initialize provider
    let options = ProviderOptions::new(CLIENT_SECRET).with_confidence_materialization_store();
    let provider = ConfidenceProvider::new(options).expect("Failed to create provider");

    let mut ofe = OpenFeature::singleton_mut().await;
    ofe.set_provider(provider).await;
    drop(ofe);

    let client = OpenFeature::singleton().await.create_client();

    // Test: should resolve a boolean
    {
        let result = client
            .get_bool_value("web-sdk-e2e-flag.bool", Some(&context()), None)
            .await
            .expect("Failed to resolve bool");

        assert!(!result, "Expected bool to be false");
    }

    // Test: should resolve an int
    {
        let result = client
            .get_int_value("web-sdk-e2e-flag.int", Some(&context()), None)
            .await
            .expect("Failed to resolve int");

        assert_eq!(result, 3, "Expected int to be 3");
    }

    // Test: should resolve a double
    {
        let result = client
            .get_float_value("web-sdk-e2e-flag.double", Some(&context()), None)
            .await
            .expect("Failed to resolve double");

        assert!(
            (result - 3.5).abs() < f64::EPSILON,
            "Expected double to be 3.5, got {}",
            result
        );
    }

    // Test: should resolve a string
    {
        let result = client
            .get_string_value("web-sdk-e2e-flag.str", Some(&context()), None)
            .await
            .expect("Failed to resolve string");

        assert_eq!(result, "control", "Expected string to be 'control'");
    }

    // Test: should resolve a struct
    {
        let result = client
            .get_struct_value::<StructValue>("web-sdk-e2e-flag.obj", Some(&context()), None)
            .await
            .expect("Failed to resolve struct");

        // Check individual fields
        assert_eq!(
            result.fields.get("int"),
            Some(&Value::Float(4.0)),
            "Expected obj.int to be 4"
        );
        assert_eq!(
            result.fields.get("str"),
            Some(&Value::String("obj control".to_string())),
            "Expected obj.str to be 'obj control'"
        );
        assert_eq!(
            result.fields.get("bool"),
            Some(&Value::Bool(false)),
            "Expected obj.bool to be false"
        );

        // Check double with tolerance
        if let Some(Value::Float(d)) = result.fields.get("double") {
            assert!(
                (*d - 3.6).abs() < 0.001,
                "Expected obj.double to be 3.6, got {}",
                d
            );
        } else {
            panic!("Expected obj.double to be a float");
        }

        // Check nested obj-obj exists and is empty struct
        assert!(
            result.fields.contains_key("obj-obj"),
            "Expected obj-obj to exist"
        );
    }

    // Test: should resolve a sub value from a struct
    {
        let result = client
            .get_bool_value("web-sdk-e2e-flag.obj.bool", Some(&context()), None)
            .await
            .expect("Failed to resolve sub-value bool");

        assert!(!result, "Expected obj.bool to be false");
    }

    // Test: should resolve a sub value from a struct with details
    {
        let result = client
            .get_float_details("web-sdk-e2e-flag.obj.double", Some(&context()), None)
            .await
            .expect("Failed to resolve sub-value double details");

        assert!(
            (result.value - 3.6).abs() < 0.001,
            "Expected value to be 3.6, got {}",
            result.value
        );
        assert_eq!(
            result.variant,
            Some("flags/web-sdk-e2e-flag/variants/control".to_string()),
            "Expected variant to be control"
        );
        assert_eq!(
            result.reason,
            Some(EvaluationReason::TargetingMatch),
            "Expected reason to be TargetingMatch"
        );
    }

    // Test: should resolve a flag with a sticky resolve
    {
        let result = client
            .get_float_details("web-sdk-e2e-flag.double", Some(&sticky_context()), None)
            .await
            .expect("Failed to resolve sticky flag");

        // The flag has a running experiment with a sticky assignment.
        // The intake is paused but we should still get the sticky assignment.
        // If this test breaks it could mean that the experiment was removed
        // or that the bigtable materialization was cleaned out.
        assert!(
            (result.value - 99.99).abs() < 0.001,
            "Expected sticky value to be 99.99, got {}",
            result.value
        );
        assert_eq!(
            result.variant,
            Some("flags/web-sdk-e2e-flag/variants/sticky".to_string()),
            "Expected variant to be sticky"
        );
        assert_eq!(
            result.reason,
            Some(EvaluationReason::TargetingMatch),
            "Expected reason to be TargetingMatch"
        );
    }

    // Test: should return caller's default when flag doesn't exist
    // This verifies the .unwrap_or() pattern works correctly
    {
        let bool_default = client
            .get_bool_value("nonexistent-flag", Some(&context()), None)
            .await
            .unwrap_or(true);
        assert!(bool_default, "Expected caller's default of true");

        let int_default = client
            .get_int_value("nonexistent-flag", Some(&context()), None)
            .await
            .unwrap_or(999);
        assert_eq!(int_default, 999, "Expected caller's default of 999");

        let string_default = client
            .get_string_value("nonexistent-flag", Some(&context()), None)
            .await
            .unwrap_or_else(|_| "my-fallback".to_string());
        assert_eq!(
            string_default, "my-fallback",
            "Expected caller's default string"
        );
    }

    // Shutdown
    OpenFeature::singleton_mut().await.shutdown().await;
}
