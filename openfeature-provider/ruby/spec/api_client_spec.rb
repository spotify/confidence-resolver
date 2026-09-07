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
end
