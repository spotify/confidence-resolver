require "confidence/openfeature"
require "rspec"
require "json"

RSpec.describe "event tracking" do
  describe Confidence::OpenFeature::APIClient do
    let(:requests) { [] }

    subject {
      Confidence::OpenFeature::APIClient.new(client_secret: "sekret")
    }

    def stub_events_response(code:, body:)
      allow_any_instance_of(Net::HTTP).to receive(:request) do |_agent, request|
        requests << request
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

    it "delegates to the api client" do
      at = Time.at(0)
      subject.track(event_name: "my-event", payload: {"a" => 1}, event_time: at)

      expect(stub_api_client.calls).to eq([["my-event", {"a" => 1}, at]])
    end

    it "defaults the payload and event time" do
      subject.track(event_name: "my-event")

      expect(stub_api_client.calls).to eq([["my-event", {}, nil]])
    end

    it "propagates rejections to the caller" do
      stub_api_client.raise_with = Confidence::OpenFeature::EventPublishError.new(
        "refused",
        [Confidence::OpenFeature::Rejection.new(0, "EVENT_SCHEMA_VALIDATION_FAILED", "bad")]
      )

      expect { subject.track(event_name: "my-event") }.to raise_error(
        Confidence::OpenFeature::EventPublishError
      )
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
