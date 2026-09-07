# Confidence OpenFeature Ruby Provider

This repo contains the OpenFeature Ruby flag provider for [Confidence](https://confidence.spotify.com/).

## Architecture

**Note:** This provider uses the **online resolver** approach. Flag evaluations are resolved by making API calls to the Confidence backend for each evaluation request. This is different from the local resolver approach used in other Confidence providers (Java, JavaScript, Go) which use WebAssembly (WASM) for local flag evaluation.

## OpenFeature

Before starting to use the provider, it can be helpful to read through the general [OpenFeature docs](https://docs.openfeature.dev/)
and get familiar with the concepts. 

## Support Matrix

This library supports the same platforms as the [OpenFeature Ruby SDK](https://github.com/open-feature/ruby-sdk).

## Installation

Install the gem and add to the application's `Gemfile` by executing:

```sh
bundle add confidence-openfeature-provider
```

If bundler is not being used to manage dependencies, install the gem by executing:

```sh
gem install confidence-openfeature-provider
```

## Getting Your Credentials

You'll need a **client secret** from Confidence to use this provider.

**📖 See the [Integration Guide: Getting Your Credentials](../INTEGRATION_GUIDE.md#getting-your-credentials)** for step-by-step instructions on:
- How to navigate the Confidence dashboard
- Creating a Backend integration
- Creating a test flag for verification
- Best practices for credential storage

## Creating and Using the Flag Provider

Below is an example for how to create a OpenFeature client using the Confidence flag provider, and then resolve a flag with a boolean attribute. The provider is configured with a **client secret** and a region, which will determine where it will send the resolving requests.

The flag will be applied immediately, meaning that Confidence will count the targeted user as having received the treatment. 

You can retrieve attributes on the flag variant using property dot notation, meaning `test-flag.boolean-key` will retrieve the attribute `boolean-key` on the flag `test-flag`. 

You can also use only the flag name `test-flag` and retrieve all values as a map with `resolve_object_details()`. 

The flag's schema is validated against the requested data type, and if it doesn't match it will fall back to the default value.

```ruby
require "openfeature/sdk"
require "confidence/openfeature"

# Configure OpenFeature with Confidence provider
OpenFeature::SDK.configure do |config|
  api_client = Confidence::OpenFeature::APIClient.new(
    client_secret: ENV['CONFIDENCE_CLIENT_SECRET'], # Get from Confidence dashboard
    region: Confidence::OpenFeature::Region::EU
  )
  config.provider = Confidence::OpenFeature::Provider.new(api_client: api_client)
end

# Create a client
open_feature_client = OpenFeature::SDK.build_client(name: "my-app")

ctx = OpenFeature::SDK::EvaluationContext.new(
  targeting_key: "random",
  attributes: {user: {country: "SE"}}
)

flag_value = open_feature_client.fetch_boolean_value(
  flag_key: "test-flag.boolean-key",
  default_value: false,
  evaluation_context: ctx
)

print(flag_value)
```

### Ruby on Rails Example

For a complete Rails integration example that you can run, see [`examples/rails_example.rb`](examples/rails_example.rb).

Quick snippet:

```ruby
# config/initializers/confidence.rb
require "openfeature/sdk"
require "confidence/openfeature"

OpenFeature::SDK.configure do |config|
  api_client = Confidence::OpenFeature::APIClient.new(
    client_secret: ENV['CONFIDENCE_CLIENT_SECRET'],
    region: Confidence::OpenFeature::Region::EU
  )
  config.provider = Confidence::OpenFeature::Provider.new(api_client: api_client)
end

# app/controllers/application_controller.rb
class ApplicationController < ActionController::Base
  def feature_enabled?(flag_key, default: false)
    client = OpenFeature::SDK.build_client(name: "rails-app")
    ctx = OpenFeature::SDK::EvaluationContext.new(
      targeting_key: current_user&.id || session.id,
      attributes: {user: {country: request.location&.country_code}}
    )
    client.fetch_boolean_value(
      flag_key: flag_key,
      default_value: default,
      evaluation_context: ctx
    )
  end
end
```

## Evaluation Context

The evaluation context contains information about the user/session being evaluated for targeting and A/B testing.

### Ruby-Specific Examples

```ruby
# Simple attributes
context = OpenFeature::SDK::EvaluationContext.new(
  targeting_key: "user-123",
  attributes: {
    country: "US",
    plan: "premium",
    age: 25
  }
)
```

## Error Handling

⚠️ **Important**: This provider uses the **online resolver** approach - each flag evaluation makes a network call to Confidence. Proper error handling is critical!

**📖 See the [Integration Guide: Error Handling](../INTEGRATION_GUIDE.md#error-handling)** for:
- Common failure scenarios
- Error codes and meanings
- Production best practices
- Monitoring recommendations

### Ruby-Specific Examples (Online Resolver)

```ruby
# The provider returns the default value on errors (including network failures)
details = client.fetch_boolean_details(
  flag_key: "test-flag.enabled",
  default_value: false,
  evaluation_context: context
)

if details.error_code
  Rails.logger.error("Flag error: #{details.error_message} (#{details.reason})")
end
```

## Event Tracking

Events are published to the Confidence events API. `track` follows the
OpenFeature tracking specification (requirement 6.1.1.1), so it takes the
tracking event name positionally and returns nothing:

```ruby
provider.track(
  "checkout-completed",
  evaluation_context: ::OpenFeature::SDK::EvaluationContext.new(targeting_key: "user-1"),
  tracking_event_details: {"value" => 12.5, "cart_size" => 3}
)
```

The event name is the event definition id and is sent as
`eventDefinitions/<name>`.

### Payload shape

Matching the other Confidence SDKs, `tracking_event_details` become top-level
payload fields and the evaluation context is nested under a reserved `context`
key:

```json
{ "value": 12.5, "cart_size": 3, "context": { "targeting_key": "user-1" } }
```

Per the spec, `value` is an optional numeric field and other custom fields may
be `boolean | string | number | structure`. Details keys are stringified. An
`event_time` entry (a `Time`) backdates the event and is removed from the
payload rather than published as a custom field.

Because details may not shadow the context, passing a `context` key raises
`InvalidContextInPayloadError` rather than silently overwriting one with the
other.

### Error handling: `track` vs `track!`

`track` never raises. The OpenFeature SDK client does not rescue around
tracking, so an exception would surface in application code from a
fire-and-forget call; failures are written to stderr instead.

Use `track!` when you want failures raised:

```ruby
begin
  provider.track!("checkout-completed", tracking_event_details: {"cart_size" => 3})
rescue Confidence::OpenFeature::EventPublishError => e
  e.rejections.each do |rejection|
    Rails.logger.error("event #{rejection.index} refused: #{rejection.reason}")
  end
end
```

- `APIError` if the request itself fails, for example an invalid client secret.
- `EventPublishError` if the batch is accepted but the event is refused. The
  events API returns HTTP 200 in that case, so these rejections would otherwise
  be invisible. Inspect `rejections` for the index, reason (such as
  `EVENT_DEFINITION_NOT_FOUND` or `EVENT_SCHEMA_VALIDATION_FAILED`) and message.
- `InvalidContextInPayloadError` on a reserved-key collision, and
  `TypeMismatchError` if `value` is not numeric.

### Calling through the OpenFeature client

Client-level tracking (`client.track(...)`) was added in openfeature-sdk 0.6.1.
This gem currently pins `~> 0.4.1`, so call `track` on the provider directly
until that pin is raised. The signature already matches what the SDK client
invokes, so no code change will be needed then.

## Shutdown

**Important**: The provider makes network calls for each flag evaluation. Always ensure proper shutdown to close connections gracefully.

```ruby
# In your application shutdown handler (e.g., config/puma.rb)
on_worker_shutdown do
  OpenFeature::SDK.shutdown
  Rails.logger.info("Confidence provider shutdown completed")
end

# For non-Rails applications
at_exit do
  OpenFeature::SDK.shutdown
end
```

### Why Shutdown Matters

For the online resolver provider:
- **Closes HTTP connections** to Confidence API
- **Prevents connection leaks** in long-running processes
- **Ensures graceful termination** of pending requests

### Ruby on Rails Considerations
In multi-process servers (Puma, Unicorn), each worker process should shut down its own provider instance.

```ruby
# config/puma.rb
on_worker_shutdown do
  ActiveSupport::Notifications.instrument("shutdown.confidence") do
    OpenFeature::SDK.shutdown
  end
end

# config/environments/production.rb
config.after_initialize do
  at_exit do
    OpenFeature::SDK.shutdown
  end
end
```
