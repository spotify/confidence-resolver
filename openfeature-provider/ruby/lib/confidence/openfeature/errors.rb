# frozen_string_literal: true

module Confidence
  module OpenFeature
    class BaseError < StandardError
    end

    class APIError < BaseError
    end

    class FlagNotFoundError < BaseError
    end

    class TypeMismatchError < BaseError
    end

    # Raised when tracking event details already carry a "context" key.
    #
    # The evaluation context is merged into the event payload under the
    # reserved "context" key, so a details key of the same name would be
    # ambiguous. Rejecting it matches the other Confidence SDKs, which raise
    # rather than silently overwrite one with the other.
    class InvalidContextInPayloadError < BaseError
    end

    # Raised when the events API accepts the batch but refuses individual
    # events. The batch call still returns HTTP 200 in that case, so without
    # this the rejections would be silently dropped.
    #
    # +rejections+ holds one Rejection per refused event, each carrying the
    # index of the event in the published batch, the reason (for example
    # EVENT_DEFINITION_NOT_FOUND or EVENT_SCHEMA_VALIDATION_FAILED) and an
    # optional message.
    class EventPublishError < BaseError
      attr_reader :rejections

      def initialize(message, rejections = [])
        super(message)
        @rejections = rejections
      end
    end
  end
end
