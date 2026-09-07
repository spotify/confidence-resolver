# frozen_string_literal: true

require_relative "api_client"
require "open_feature/sdk"

module Confidence
  module OpenFeature
    # See NoOpProvider in the OpenFeature Ruby SDK for the interface
    class Provider
      attr_reader :metadata

      # Error_code and error_message seemingly not used by OpenFeature SDK.
      # Including here for compatibility.
      ResolutionDetails = Struct.new(
        :value, :reason, :variant, :error_code, :error_message
      )

      def initialize(api_client:, apply_on_resolve: true)
        @api_client = api_client
        @apply_on_resolve = apply_on_resolve
        @metadata = ::OpenFeature::SDK::Provider::ProviderMetadata.new(name: "Confidence").freeze
      end

      def fetch_boolean_value(flag_key:, default_value:, evaluation_context: nil)
        evaluate(
          flag_key: flag_key,
          default_value: default_value,
          evaluation_context: evaluation_context,
          validator: lambda { |v| v === true || v === false }
        )
      end

      def fetch_string_value(flag_key:, default_value:, evaluation_context: nil)
        evaluate(
          flag_key: flag_key,
          default_value: default_value,
          evaluation_context: evaluation_context,
          validator: lambda { |v| v.is_a?(String) }
        )
      end

      def fetch_number_value(flag_key:, default_value:, evaluation_context: nil)
        evaluate(
          flag_key: flag_key,
          default_value: default_value,
          evaluation_context: evaluation_context,
          validator: lambda { |v| v.is_a?(Numeric) }
        )
      end

      def fetch_object_value(flag_key:, default_value:, evaluation_context: nil)
        evaluate(
          flag_key: flag_key,
          default_value: default_value,
          evaluation_context: evaluation_context
        )
      end

      # Publishes an event to Confidence.
      #
      # Signature matches OpenFeature requirement 6.1.1.1 and the shape the
      # OpenFeature Ruby SDK client invokes providers with:
      #
      #   @provider.track(name, evaluation_context:, tracking_event_details:)
      #
      # Returns nothing, and never raises: the SDK client does not rescue, so
      # an exception here would surface in application code from a
      # fire-and-forget tracking call. Failures are written to stderr. Use
      # +track!+ when you want them raised instead.
      def track(tracking_event_name, evaluation_context: nil, tracking_event_details: nil)
        track!(
          tracking_event_name,
          evaluation_context: evaluation_context,
          tracking_event_details: tracking_event_details
        )
        nil
      rescue => ex
        # Bare rescue is StandardError; anything narrower would let a caller
        # mistake (a non-Hash, say) escape a call that must not raise.
        warn("Confidence: track(#{tracking_event_name.inspect}) failed: #{ex.message}")
        nil
      end

      # Same as +track+ but raises on failure.
      #
      # Raises APIError if the request fails, EventPublishError if the batch is
      # accepted but the event is refused, InvalidContextInPayloadError on a
      # reserved-key collision and TypeMismatchError if +value+ is not numeric.
      #
      # +event_time+ backdates the event. It can also be supplied as an
      # "event_time" entry in +tracking_event_details+, which is the only route
      # available through the spec-conformant +track+; it is removed from the
      # payload rather than published as a custom field.
      def track!(tracking_event_name, evaluation_context: nil, tracking_event_details: nil, event_time: nil)
        details = normalize_details(tracking_event_details)
        at = details.delete("event_time") || event_time

        @api_client.track(
          event_name: tracking_event_name,
          payload: event_payload(details, evaluation_context),
          event_time: at
        )
      end

      private

      # Mirrors PayloadMerger in the other Confidence SDKs: the tracking event
      # details sit at the top level of the payload and the evaluation context
      # is nested under the reserved "context" key.
      def event_payload(details, evaluation_context)
        if details.key?("context")
          raise InvalidContextInPayloadError.new(
            'tracking event details may not contain a "context" key; it is ' \
            "reserved for the evaluation context"
          )
        end
        details.merge("context" => context_hash(evaluation_context))
      end

      # Requirement 6.2.1: tracking event details define an optional numeric
      # +value+. Requirement 6.2.2: custom fields keyed by string.
      def normalize_details(tracking_event_details)
        return {} if tracking_event_details.nil?
        unless tracking_event_details.is_a?(Hash)
          raise TypeMismatchError.new(
            "tracking event details must be a Hash, got #{tracking_event_details.class}"
          )
        end

        details = tracking_event_details.transform_keys(&:to_s)
        value = details["value"]
        if !value.nil? && !value.is_a?(Numeric)
          raise TypeMismatchError.new(
            "tracking event details 'value' must be numeric, got #{value.class}"
          )
        end
        details
      end

      def evaluate(flag_key:, default_value:, evaluation_context: nil, validator: nil)
        parts = flag_key.split(".")
        flag_id = parts.shift
        value_path = parts
        context = context_hash(evaluation_context)

        result = @api_client.resolve_one(
          flag: "flags/#{flag_id}",
          context: context,
          apply: @apply_on_resolve
        )
        if result.empty?
          return ResolutionDetails.new(
            value: default_value,
            reason: "DEFAULT"
          )
        end

        value = value_at_path(flag_key, result.value, value_path)
        if !value.nil? && validator && !validator.call(value)
          raise TypeMismatchError.new("value did not match expected type")
        end
        value = default_value if value.nil?

        ResolutionDetails.new(
          value: value,
          variant: Confidence::OpenFeature.parse_variant(result.variant),
          reason: "TARGETING_MATCH"
        )
      end

      def value_at_path(flag, value, path)
        return value if path.empty?
        the_value = value
        path.each do |key|
          if the_value.is_a?(Hash) && the_value.has_key?(key)
            the_value = the_value[key]
          else
            raise TypeMismatchError.new("#{flag}: invalid path: #{path.join(".")}")
          end
        end
        the_value
      end

      def context_hash(evaluation_context)
        return {} if evaluation_context.nil?
        # Direct callers may pass a plain Hash; the SDK client always passes an
        # EvaluationContext.
        return evaluation_context.dup if evaluation_context.is_a?(Hash)

        # In SDK 0.4+, EvaluationContext stores all fields in a hash
        # targeting_key is a special field that can be accessed via .targeting_key
        return evaluation_context.fields.dup if evaluation_context.respond_to?(:fields)

        {}
      end
    end

    def self.parse_variant(value)
      components = value.split("/", 4)
      if components.length != 4 || components[0] != "flags" || components[2] != "variants"
        raise ArgumentError.new("Invalid variant name: #{value}")
      end
      components[3]
    end
  end
end
