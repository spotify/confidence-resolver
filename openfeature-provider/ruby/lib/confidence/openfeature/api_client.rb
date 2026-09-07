# frozen_string_literal: true

require_relative "errors"
require_relative "version"
require "json"
require "uri"
require "net/http"
require "net/https"
# Time.iso8601, used to coerce a String event_time.
require "time"

module Confidence
  module OpenFeature
    class Region
      attr_reader :uri

      def initialize(uri)
        @uri = uri
      end

      EU = new("https://resolver.eu.confidence.dev/v1")
      US = new("https://resolver.us.confidence.dev/v1")
    end

    # The events API is a single global endpoint, unlike the regional
    # resolver hosts above.
    EVENTS_URI = "https://events.confidence.dev/v1"

    class APIClient
      def initialize(client_secret:, region: Region::EU, events_uri: EVENTS_URI)
        @client_secret = client_secret
        @uri = URI.parse(region.uri)
        @events_uri = URI.parse(events_uri)
      end

      def resolve_one(flag:, context: {}, apply: true)
        result = resolve(flags: [flag], context: context, apply: apply)
        if result.empty?
          raise FlagNotFoundError.new("No active flag '#{flag}' found was found")
        end
        result = result[0]
        if result.flag != flag
          raise FlagNotFoundError.new("Unexpected flag '#{flag}' from remote")
        end
        result
      end

      # Publishes a single event to the Confidence events API.
      #
      # +event_name+ is the event definition id, sent as
      # "eventDefinitions/#{event_name}". +payload+ is an arbitrary hash.
      # +event_time+ defaults to now; pass it to backdate an event.
      #
      # Returns nil on success. Raises APIError if the request itself fails
      # and EventPublishError if the batch is accepted but the event is
      # refused, so rejections are never silently discarded.
      def track(event_name:, payload: {}, event_time: nil)
        now = Time.now
        result = post_json("/v1/events:publish", {
          clientSecret: @client_secret,
          sendTime: rfc3339(now),
          sdk: {id: "SDK_ID_RUBY_PROVIDER", version: VERSION},
          events: [{
            eventDefinition: "eventDefinitions/#{event_name}",
            eventTime: rfc3339(event_time || now),
            payload: payload || {}
          }]
        }, uri: @events_uri, label: "events:publish")

        rejections = (result["errors"] || []).map do |error|
          Rejection.new(error["index"], error["reason"], error["message"])
        end
        unless rejections.empty?
          raise EventPublishError.new(
            "events:publish refused #{rejections.length} event(s): " +
              rejections.map { |r| "[#{r.index}] #{r.reason} #{r.message}".strip }.join(", "),
            rejections
          )
        end
        nil
      end

      def resolve(flags: [], context: {}, apply: true)
        result = post_json("/v1/flags:resolve", {
          clientSecret: @client_secret,
          evaluationContext: context || {},
          apply: apply,
          flags: flags,
          sdk: {id: "SDK_ID_RUBY_PROVIDER", version: VERSION}
        }, uri: @uri, label: "flags:resolve")

        resolved_flags = result["resolvedFlags"] || []
        resolved_flags.map do |flag|
          ResolvedFlag.new(
            flag: flag["flag"],
            variant: nil_if_empty(flag["variant"]),
            value: nil_if_empty(flag["value"])
          )
        end
      end

      private

      # A fresh Net::HTTP per request, because one instance cannot be shared
      # across threads. Net::HTTP#request auto-starts the connection when the
      # receiver is not already started, mutating its @started and @socket:
      #
      #   unless started?
      #     start { req['connection'] ||= 'close'; return request(req, ...) }
      #   end
      #
      # Two threads entering that on the same object both open a connection and
      # both assign @socket, so one clobbers the other and the loser reads or
      # writes a socket the winner may already have closed.
      #
      # Per-request instantiation costs nothing here: the instance was never
      # explicitly started, so every request already opened a connection, sent
      # "Connection: close" and closed it again. There was no reuse to lose.
      # Takes a URI so an additional endpoint can share it.
      def build_agent(uri)
        agent = Net::HTTP.new(uri.host, uri.port)
        agent.use_ssl = uri.scheme == "https"
        agent
      end

      # getutc rather than utc: the latter mutates its receiver, which would
      # convert a caller-supplied event_time to UTC in place.
      #
      # Accepts a String as well as a Time. Spec 6.2.2 permits string custom
      # fields, and an "event_time" entry in tracking event details is the only
      # route to set the event time through the spec-conformant +track+, so a
      # caller passing an ISO-8601 string is expected rather than exceptional.
      # An unparseable value raises TypeMismatchError rather than falling back
      # to "now": a silently wrong timestamp is harder to diagnose than a
      # logged failure, and +track+ turns the raise into a warning.
      def rfc3339(time)
        coerce_time(time).getutc.strftime("%Y-%m-%dT%H:%M:%S.%LZ")
      end

      def coerce_time(time)
        return time if time.is_a?(Time)

        if time.is_a?(String)
          begin
            return Time.iso8601(time)
          rescue ArgumentError => ex
            raise TypeMismatchError.new(
              "event_time #{time.inspect} is not a valid ISO-8601 timestamp: #{ex.message}"
            )
          end
        end

        raise TypeMismatchError.new(
          "event_time must be a Time or an ISO-8601 String, got #{time.class}"
        )
      end

      # Takes the target URI rather than a prepared agent so that every request
      # still builds its own, per build_agent above. +label+ names the endpoint
      # in errors; both callers pass it explicitly, which keeps "which host does
      # this go to" a decision at the call site.
      def post_json(path, body, uri:, label:)
        headers = {"Content-Type" => "application/json"}
        request = Net::HTTP::Post.new(path, headers)
        request.body = JSON.dump(body)
        response = build_agent(uri).request(request)

        code = response.code.to_i
        if code != 200
          raise APIError.new("#{label} HTTP #{response.code} #{response.message}")
        end

        begin
          JSON.parse(response.body)
        rescue JSON::ParserError => ex
          raise APIError.new("#{label} malformed JSON: #{ex}")
        end
      end

      def nil_if_empty(value)
        (value.nil? || value == "") ? nil : value
      end
    end

    ResolvedFlag = Struct.new(:flag, :variant, :value) do
      def empty?
        variant.nil? || value.nil?
      end
    end

    # A single event the events API refused within an accepted batch.
    Rejection = Struct.new(:index, :reason, :message)
  end
end

if __FILE__ == $0
  require "optparse"
  context = {}
  secret = nil
  OptionParser.new do |opts|
    opts.on("--secret=VALUE") do |v|
      secret = v
    end
    opts.on("--context=VALUE") do |v|
      context.merge!(JSON.parse(v))
    end
    opts.on("--targeting_key=VALUE") do |v|
      context["targeting_key"] = v
    end
  end.parse!

  client = Confidence::OpenFeature::APIClient.new(client_secret: secret)
  client.resolve(flags: ARGV, context: context).each do |flag|
    puts flag.to_h
  end
end
