# frozen_string_literal: true

require_relative "errors"
require_relative "version"
require "json"
require "uri"
require "net/http"
require "net/https"

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

    class APIClient
      def initialize(client_secret:, region: Region::EU)
        uri = URI.parse(region.uri)
        @client_secret = client_secret
        @agent = Net::HTTP.new(uri.host, uri.port)
        @agent.use_ssl = uri.scheme == "https"
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

      def resolve(flags: [], context: {}, apply: true)
        result = post_json("/v1/flags:resolve", {
          clientSecret: @client_secret,
          evaluationContext: context || {},
          apply: apply,
          flags: flags,
          sdk: {id: "SDK_ID_RUBY_PROVIDER", version: VERSION}
        })

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

      def post_json(path, body)
        headers = {"Content-Type" => "application/json"}
        request = Net::HTTP::Post.new(path, headers)
        request.body = JSON.dump(body)
        response = @agent.request(request)

        code = response.code.to_i
        if code != 200
          raise APIError.new("flags:resolve HTTP #{response.code} #{response.message}")
        end

        begin
          JSON.parse(response.body)
        rescue JSON::ParserError => ex
          raise APIError.new("flags:resolve malformed JSON: #{ex}")
        end
      end

      def nil_if_empty(value)
        (value.nil? || value == "") ? nil : value
      end
    end

    ResolvedFlag = Struct.new(:flag, :variant, :value, keyword_init: true) do
      def empty?
        variant.nil? || value.nil?
      end
    end
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
