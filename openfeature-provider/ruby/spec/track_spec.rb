require "confidence/openfeature"
require "rspec"
require "json"

RSpec.describe "event tracking" do
  describe Confidence::OpenFeature::APIClient do
    let(:requests) { [] }
    # Net::HTTP::Post carries only a path, so the request alone cannot show
    # which endpoint was used. The agent instance holds the host, and the host
    # is the only difference between the events and resolver endpoints — so
    # capture it. Without this, a regression posting events to the resolver
    # host would satisfy every other assertion here.
    let(:hosts) { [] }

    subject {
      Confidence::OpenFeature::APIClient.new(client_secret: "sekret")
    }

    def stub_events_response(code:, body:)
      allow_any_instance_of(Net::HTTP).to receive(:request) do |agent, request|
        requests << request
        hosts << agent.address
        FakeHTTPResponse.new(code: code, body: body)
      end
    end

    it "publishes to the events endpoint" do
      stub_events_response(code: 200, body: '{"errors":[]}')

      expect(subject.track(event_name: "my-event", payload: {"a" => 1})).to be_nil

      expect(requests.length).to eq(1)
      expect(requests.first.path).to eq("/v1/events:publish")
      expect(requests.first["Content-Type"]).to eq("application/json")
    end

    it "publishes to the events host, not the regional resolver host" do
      stub_events_response(code: 200, body: '{"errors":[]}')

      subject.track(event_name: "my-event")

      expect(hosts).to eq(["events.confidence.dev"])
    end

    it "accepts an ISO-8601 String event_time" do
      stub_events_response(code: 200, body: '{"errors":[]}')

      subject.track(event_name: "my-event", event_time: "1970-01-01T00:00:00.000Z")

      expect(requests.length).to eq(1)
      body = JSON.parse(requests.first.body)
      expect(body["events"].first["eventTime"]).to eq("1970-01-01T00:00:00.000Z")
    end

    it "converts a String event_time with an offset to UTC" do
      stub_events_response(code: 200, body: '{"errors":[]}')

      subject.track(event_name: "my-event", event_time: "1970-01-01T01:00:00+01:00")

      body = JSON.parse(requests.first.body)
      expect(body["events"].first["eventTime"]).to eq("1970-01-01T00:00:00.000Z")
    end

    it "raises TypeMismatchError on an unparseable String event_time" do
      stub_events_response(code: 200, body: '{"errors":[]}')

      expect {
        subject.track(event_name: "my-event", event_time: "last tuesday")
      }.to raise_error(
        Confidence::OpenFeature::TypeMismatchError, /not a valid ISO-8601 timestamp/
      )
      expect(requests).to be_empty
    end

    it "raises TypeMismatchError on a non-Time, non-String event_time" do
      stub_events_response(code: 200, body: '{"errors":[]}')

      expect {
        subject.track(event_name: "my-event", event_time: 12345)
      }.to raise_error(
        Confidence::OpenFeature::TypeMismatchError, /must be a Time or an ISO-8601 String/
      )
      expect(requests).to be_empty
    end

    it "sends the event definition, payload and sdk info" do
      stub_events_response(code: 200, body: '{"errors":[]}')

      subject.track(event_name: "my-event", payload: {"a" => 1})

      body = JSON.parse(requests.first.body)
      expect(body["clientSecret"]).to eq("sekret")
      expect(body["sdk"]).to eq({
        "id" => "SDK_ID_RUBY_PROVIDER",
        "version" => Confidence::OpenFeature::VERSION
      })
      expect(body["events"].length).to eq(1)
      expect(body["events"].first["eventDefinition"]).to eq("eventDefinitions/my-event")
      expect(body["events"].first["payload"]).to eq({"a" => 1})
    end

    it "defaults the payload to an empty hash" do
      stub_events_response(code: 200, body: '{"errors":[]}')

      subject.track(event_name: "my-event")

      body = JSON.parse(requests.first.body)
      expect(body["events"].first["payload"]).to eq({})
    end

    it "sends RFC3339 UTC timestamps" do
      stub_events_response(code: 200, body: '{"errors":[]}')

      subject.track(event_name: "my-event", event_time: Time.at(0))

      body = JSON.parse(requests.first.body)
      expect(body["events"].first["eventTime"]).to eq("1970-01-01T00:00:00.000Z")
      expect(body["sendTime"]).to match(/\A\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z\z/)
    end

    it "does not mutate a caller-supplied event time" do
      stub_events_response(code: 200, body: '{"errors":[]}')
      at = Time.at(0)
      was_utc = at.utc?

      subject.track(event_name: "my-event", event_time: at)

      expect(at.utc?).to eq(was_utc)
    end

    it "treats a missing errors array as success" do
      stub_events_response(code: 200, body: "{}")

      expect(subject.track(event_name: "my-event")).to be_nil
    end

    it "raises with rejection details when an event is refused" do
      stub_events_response(code: 200, body: <<~JSON)
        {"errors":[{"index":0,"reason":"EVENT_DEFINITION_NOT_FOUND","message":"nope"}]}
      JSON

      expect { subject.track(event_name: "missing-event") }.to raise_error(
        Confidence::OpenFeature::EventPublishError
      ) { |error|
        expect(error.rejections.length).to eq(1)
        expect(error.rejections.first.index).to eq(0)
        expect(error.rejections.first.reason).to eq("EVENT_DEFINITION_NOT_FOUND")
        expect(error.rejections.first.message).to eq("nope")
        expect(error.message).to include("EVENT_DEFINITION_NOT_FOUND")
      }
    end

    it "raises an APIError on a non-200 response" do
      stub_events_response(code: 403, body: "denied")

      expect { subject.track(event_name: "my-event") }.to raise_error(
        Confidence::OpenFeature::APIError, /events:publish HTTP 403/
      )
    end

    it "raises an APIError on malformed JSON" do
      stub_events_response(code: 200, body: "not json")

      expect { subject.track(event_name: "my-event") }.to raise_error(
        Confidence::OpenFeature::APIError, /events:publish malformed JSON/
      )
    end
  end

  describe Confidence::OpenFeature::Provider do
    let(:stub_api_client) { TrackStubAPIClient.new }

    subject {
      Confidence::OpenFeature::Provider.new(api_client: stub_api_client)
    }

    def last_payload
      stub_api_client.calls.last[1]
    end

    # OpenFeature 6.1.1.1: tracking event name (required), evaluation context
    # (optional), tracking event details (optional).
    describe "OpenFeature conformance" do
      it "takes the tracking event name as a required positional parameter" do
        params = Confidence::OpenFeature::Provider.instance_method(:track).parameters

        expect(params).to include([:req, :tracking_event_name])
        expect(params).to include([:key, :evaluation_context])
        expect(params).to include([:key, :tracking_event_details])
      end

      it "accepts the exact invocation the SDK client uses" do
        context = ::OpenFeature::SDK::EvaluationContext.new(targeting_key: "user-1")

        expect {
          subject.track(
            "my-event",
            evaluation_context: context,
            tracking_event_details: {"value" => 1}
          )
        }.not_to raise_error
      end

      it "routes through the real OpenFeature SDK client" do
        sdk_client = ::OpenFeature::SDK::Client.new(provider: subject)

        sdk_client.track("my-event", tracking_event_details: {"value" => 3})

        expect(stub_api_client.calls.length).to eq(1)
        event_name, payload, _at = stub_api_client.calls.first
        expect(event_name).to eq("my-event")
        expect(payload["value"]).to eq(3)
      end

      it "passes the merged evaluation context through the SDK client" do
        sdk_client = ::OpenFeature::SDK::Client.new(
          provider: subject,
          evaluation_context: ::OpenFeature::SDK::EvaluationContext.new(client_key: "client_value")
        )

        sdk_client.track(
          "my-event",
          evaluation_context: ::OpenFeature::SDK::EvaluationContext.new(invocation_key: "invocation_value")
        )

        context = stub_api_client.calls.first[1]["context"]
        expect(context["client_key"]).to eq("client_value")
        expect(context["invocation_key"]).to eq("invocation_value")
      end

      # 6.1.4: the client no-ops when the provider does not implement tracking.
      it "no-ops for a provider without tracking" do
        bare = Class.new {
          def metadata
            ::OpenFeature::SDK::Provider::ProviderMetadata.new(name: "bare")
          end
        }.new

        expect { ::OpenFeature::SDK::Client.new(provider: bare).track("my-event") }
          .not_to raise_error
      end

      it "returns nothing" do
        expect(subject.track("my-event")).to be_nil
      end
    end

    # openfeature-sdk 0.6.1 delegates flag_metadata to whatever the provider
    # returns, so ResolutionDetails must carry the member.
    describe "ResolutionDetails flag_metadata" do
      it "defaults to a frozen empty hash" do
        details = Confidence::OpenFeature::Provider::ResolutionDetails.new(value: true)

        expect(details.flag_metadata).to eq({})
        expect(details.flag_metadata).to be_frozen
      end

      it "freezes caller-supplied metadata" do
        details = Confidence::OpenFeature::Provider::ResolutionDetails.new(
          value: true, flag_metadata: {"a" => 1}
        )

        expect(details.flag_metadata).to eq({"a" => 1})
        expect(details.flag_metadata).to be_frozen
      end

      it "is reachable through EvaluationDetails delegation" do
        details = ::OpenFeature::SDK::EvaluationDetails.new(
          flag_key: "flag",
          resolution_details: Confidence::OpenFeature::Provider::ResolutionDetails.new(value: true)
        )

        expect(details.flag_metadata).to eq({})
      end
    end

    it "puts details at the payload top level and context under 'context'" do
      context = ::OpenFeature::SDK::EvaluationContext.new(targeting_key: "user-1")

      subject.track(
        "my-event",
        evaluation_context: context,
        tracking_event_details: {"cart_size" => 3}
      )

      expect(last_payload["cart_size"]).to eq(3)
      expect(last_payload["context"]).to eq({"targeting_key" => "user-1"})
    end

    it "accepts a plain hash as the evaluation context" do
      subject.track("my-event", evaluation_context: {"country" => "SE"})

      expect(last_payload["context"]).to eq({"country" => "SE"})
    end

    it "sends an empty context and no custom fields when both are omitted" do
      subject.track("my-event")

      expect(last_payload).to eq({"context" => {}})
    end

    it "stringifies symbol detail keys" do
      subject.track("my-event", tracking_event_details: {cart_size: 3})

      expect(last_payload["cart_size"]).to eq(3)
    end

    # 6.2.1: tracking event details define an optional numeric value.
    it "accepts a numeric value" do
      subject.track("my-event", tracking_event_details: {"value" => 12.5})

      expect(last_payload["value"]).to eq(12.5)
    end

    it "rejects a non-numeric value" do
      expect {
        subject.track!("my-event", tracking_event_details: {"value" => "lots"})
      }.to raise_error(Confidence::OpenFeature::TypeMismatchError, /must be numeric/)
    end

    it "rejects details carrying the reserved context key" do
      expect {
        subject.track!("my-event", tracking_event_details: {"context" => {"a" => 1}})
      }.to raise_error(Confidence::OpenFeature::InvalidContextInPayloadError, /reserved/)
    end

    it "takes event_time from details without publishing it as a field" do
      at = Time.at(0)

      subject.track("my-event", tracking_event_details: {"event_time" => at, "a" => 1})

      expect(stub_api_client.calls.last[2]).to eq(at)
      expect(last_payload).not_to have_key("event_time")
      expect(last_payload["a"]).to eq(1)
    end

    it "accepts an explicit event_time on the raising variant" do
      at = Time.at(0)

      subject.track!("my-event", event_time: at)

      expect(stub_api_client.calls.last[2]).to eq(at)
    end

    # The specs above stub the API client, so the timestamp is never actually
    # formatted. These drive Provider#track through the real APIClient so the
    # event_time coercion runs — a String event_time previously raised
    # NoMethodError inside rfc3339, which track's rescue swallowed, dropping
    # the whole event with nothing published.
    describe "event_time through the real API client" do
      let(:requests) { [] }

      let(:real_provider) {
        Confidence::OpenFeature::Provider.new(
          api_client: Confidence::OpenFeature::APIClient.new(client_secret: "sekret")
        )
      }

      before do
        allow_any_instance_of(Net::HTTP).to receive(:request) do |_agent, request|
          requests << request
          FakeHTTPResponse.new(code: 200, body: '{"errors":[]}')
        end
      end

      def published_event
        JSON.parse(requests.first.body)["events"].first
      end

      it "publishes a String event_time from details" do
        real_provider.track(
          "my-event",
          tracking_event_details: {"event_time" => "1970-01-01T00:00:00.000Z", "a" => 1}
        )

        expect(requests.length).to eq(1), "the event was never published"
        expect(published_event["eventTime"]).to eq("1970-01-01T00:00:00.000Z")
        expect(published_event["payload"]).not_to have_key("event_time")
        expect(published_event["payload"]["a"]).to eq(1)
      end

      it "still publishes a Time event_time from details" do
        real_provider.track(
          "my-event",
          tracking_event_details: {"event_time" => Time.at(0)}
        )

        expect(requests.length).to eq(1)
        expect(published_event["eventTime"]).to eq("1970-01-01T00:00:00.000Z")
      end

      it "leaves unrelated string fields untouched" do
        real_provider.track(
          "my-event",
          tracking_event_details: {"note" => "last tuesday"}
        )

        expect(requests.length).to eq(1)
        expect(published_event["payload"]["note"]).to eq("last tuesday")
      end

      it "logs rather than raising on an unparseable event_time, publishing nothing" do
        expect {
          expect(
            real_provider.track(
              "my-event",
              tracking_event_details: {"event_time" => "last tuesday"}
            )
          ).to be_nil
        }.to output(/not a valid ISO-8601 timestamp/).to_stderr

        expect(requests).to be_empty
      end

      it "raises on an unparseable event_time through track!" do
        expect {
          real_provider.track!(
            "my-event",
            tracking_event_details: {"event_time" => "last tuesday"}
          )
        }.to raise_error(
          Confidence::OpenFeature::TypeMismatchError, /not a valid ISO-8601 timestamp/
        )

        expect(requests).to be_empty
      end
    end

    describe "never raising" do
      it "logs instead of raising when the event is refused" do
        stub_api_client.raise_with = Confidence::OpenFeature::EventPublishError.new(
          "refused",
          [Confidence::OpenFeature::Rejection.new(0, "EVENT_SCHEMA_VALIDATION_FAILED", "bad")]
        )

        expect { expect(subject.track("my-event")).to be_nil }
          .to output(/track\("my-event"\) failed: refused/).to_stderr
      end

      it "logs instead of raising when the request fails" do
        stub_api_client.raise_with = Confidence::OpenFeature::APIError.new(
          "events:publish HTTP 401 Unauthorized"
        )

        expect { expect(subject.track("my-event")).to be_nil }
          .to output(/HTTP 401/).to_stderr
      end

      it "logs instead of raising on a reserved-key collision" do
        expect {
          expect(subject.track("my-event", tracking_event_details: {"context" => {}})).to be_nil
        }.to output(/reserved/).to_stderr
      end

      it "logs instead of raising when details are not a hash" do
        expect {
          expect(subject.track("my-event", tracking_event_details: "nope")).to be_nil
        }.to output(/must be a Hash/).to_stderr
      end
    end

    describe "#track!" do
      it "raises rejections to the caller" do
        stub_api_client.raise_with = Confidence::OpenFeature::EventPublishError.new(
          "refused",
          [Confidence::OpenFeature::Rejection.new(0, "EVENT_SCHEMA_VALIDATION_FAILED", "bad")]
        )

        expect { subject.track!("my-event") }.to raise_error(
          Confidence::OpenFeature::EventPublishError
        )
      end
    end
  end
end

class FakeHTTPResponse
  attr_reader :code, :message, :body

  def initialize(code:, body:, message: "Test")
    @code = code.to_s
    @body = body
    @message = message
  end
end

class TrackStubAPIClient
  attr_accessor :calls
  attr_writer :raise_with

  def initialize
    @calls = []
    @raise_with = nil
  end

  def track(event_name:, payload:, event_time:)
    @calls << [event_name, payload, event_time]
    raise @raise_with if @raise_with
    nil
  end
end
