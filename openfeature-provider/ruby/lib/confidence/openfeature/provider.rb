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

      private

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

        # In SDK 0.4+, EvaluationContext stores all fields in a hash
        # targeting_key is a special field that can be accessed via .targeting_key
        evaluation_context.fields.dup
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
