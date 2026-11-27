# frozen_string_literal: true

require "confidence/openfeature"
require "rspec"
require "json"
require "net/http"

RSpec.describe Confidence::OpenFeature::APIClient do
  subject(:client) { described_class.new(client_secret: "secret") }

  def resolved_body(flag)
    {"resolvedFlags" => [{"flag" => flag, "variant" => "flags/#{flag}/variants/v", "value" => {"a" => 1}}]}
  end

  def ok_response(flag)
    double("Net::HTTPOK", code: "200", message: "OK", body: JSON.dump(resolved_body(flag)))
  end

  # Records every Net::HTTP instance the client builds, and stubs #request on
  # each so nothing touches the network. `before` blocks are not used because
  # each example needs a different #request behaviour.
  def capture_agents(&request_impl)
    agents = []
    allow(Net::HTTP).to receive(:new).and_wrap_original do |original, *args|
      agent = original.call(*args)
      agents << agent
      allow(agent).to receive(:request) { |req| request_impl.call(req) }
      agent
    end
    agents
  end

  # This is the deterministic guard for the thread-safety fix. A memoised,
  # shared Net::HTTP is unsafe because #request mutates @started/@socket on the
  # receiver, so the fix is "never share the instance". That structural
  # property is assertable without racing anything.
  it "builds a separate Net::HTTP for each request" do
    agents = capture_agents { |req| ok_response(JSON.parse(req.body)["flags"].first) }

    client.resolve(flags: ["flags/one"])
    client.resolve(flags: ["flags/two"])

    expect(agents.length).to eq(2)
    expect(agents.map(&:object_id).uniq.length).to eq(2)
  end

  it "configures TLS on every agent it builds" do
    agents = capture_agents { |req| ok_response(JSON.parse(req.body)["flags"].first) }

    client.resolve(flags: ["flags/one"])

    expect(agents.length).to eq(1)
    expect(agents.first.use_ssl?).to be(true)
    expect(agents.first.address).to eq("resolver.eu.confidence.dev")
  end

  # Concurrency smoke test. NOTE: this is deliberately NOT claimed as a
  # red-side guard. Stubbing #request removes the socket that the real race
  # corrupts, so this example also passes against a shared memoised agent.
  # It exists to prove the fix introduces no cross-thread bleed of its own
  # (each thread must see the response for the flag it asked for) and that no
  # exception escapes. The example above is what actually fails without the fix.
  it "gives each thread its own response under concurrency" do
    capture_agents do |req|
      flag = JSON.parse(req.body)["flags"].first
      sleep 0.01 # widen the window between building the agent and responding
      ok_response(flag)
    end

    results = {}
    errors = []
    threads = 8.times.map do |i|
      Thread.new do
        flag = "flags/f#{i}"
        results[flag] = client.resolve(flags: [flag]).first.flag
      rescue => ex
        errors << ex
      end
    end
    threads.each(&:join)

    expect(errors).to be_empty
    expect(results.length).to eq(8)
    results.each { |requested, returned| expect(returned).to eq(requested) }
  end

  describe "HTTP timeouts" do
    it "configures open_timeout, read_timeout, and write_timeout on built agents" do
      client = described_class.new(
        client_secret: "secret",
        open_timeout: 1.5,
        read_timeout: 2.5,
        write_timeout: 3.5
      )

      agents = capture_agents { |req| ok_response(JSON.parse(req.body)["flags"].first) }

      client.resolve(flags: ["flags/one"])

      expect(agents.length).to eq(1)
      expect(agents.first.open_timeout).to eq(1.5)
      expect(agents.first.read_timeout).to eq(2.5)
      expect(agents.first.write_timeout).to eq(3.5) if agents.first.respond_to?(:write_timeout)
    end

    it "leaves timeouts at Net::HTTP defaults when omitted" do
      client = described_class.new(client_secret: "secret")

      agents = capture_agents { |req| ok_response(JSON.parse(req.body)["flags"].first) }

      client.resolve(flags: ["flags/one"])

      expect(agents.length).to eq(1)
      expect(client.open_timeout).to be_nil
      expect(client.read_timeout).to be_nil
      expect(client.write_timeout).to be_nil
    end

    it "rejects non-numeric timeout values with ArgumentError" do
      expect {
        described_class.new(client_secret: "secret", open_timeout: "fast")
      }.to raise_error(ArgumentError, /open_timeout must be a positive number/)
    end

    it "rejects negative or zero timeout values with ArgumentError" do
      expect {
        described_class.new(client_secret: "secret", read_timeout: -1)
      }.to raise_error(ArgumentError, /read_timeout must be a positive number/)

      expect {
        described_class.new(client_secret: "secret", write_timeout: 0)
      }.to raise_error(ArgumentError, /write_timeout must be a positive number/)
    end

    it "wraps Net::OpenTimeout as APIError" do
      client = described_class.new(client_secret: "secret")
      capture_agents { |_req| raise Net::OpenTimeout.new("execution expired") }

      expect {
        client.resolve(flags: ["flags/one"])
      }.to raise_error(Confidence::OpenFeature::APIError, /flags:resolve timed out: execution expired/)
    end

    it "wraps Net::ReadTimeout as APIError" do
      client = described_class.new(client_secret: "secret")
      capture_agents { |_req| raise Net::ReadTimeout.new("read timed out") }

      expect {
        client.resolve(flags: ["flags/one"])
      }.to raise_error(Confidence::OpenFeature::APIError, /flags:resolve timed out: read timed out/)
    end
  end

end
