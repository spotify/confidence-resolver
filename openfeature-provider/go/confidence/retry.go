package confidence

// RetryServiceConfig is the default gRPC service config for flag log writes.
// It retries UNAVAILABLE errors with exponential backoff.
// Custom TransportHooks implementations receive this in the base dial options
// and can keep, modify, or replace it.
const RetryServiceConfig = `{
	"methodConfig": [{
		"name": [{"service": "confidence.flags.resolver.v1.InternalFlagLoggerService"}],
		"retryPolicy": {
			"maxAttempts": 3,
			"initialBackoff": "1s",
			"maxBackoff": "10s",
			"backoffMultiplier": 2.0,
			"retryableStatusCodes": ["UNAVAILABLE"]
		}
	}]
}`
