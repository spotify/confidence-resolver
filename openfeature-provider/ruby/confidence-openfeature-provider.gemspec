# frozen_string_literal: true

require_relative "lib/confidence/openfeature/version"

ROOT_URL = "https://github.com/spotify/confidence-resolver"
GITHUB_URL = "#{ROOT_URL}/blob/main/openfeature-provider/ruby"

Gem::Specification.new do |spec|
  spec.name = "confidence-openfeature-provider"
  spec.version = Confidence::OpenFeature::VERSION
  spec.authors = ["Confidence Team"]
  spec.email = ["TBD@TBD.com"]

  spec.summary = "Confidence provider for the OpenFeature SDK"
  spec.homepage = GITHUB_URL
  spec.license = "Apache-2.0'"
  spec.required_ruby_version = ">= 3.1"

  spec.metadata["homepage_uri"] = GITHUB_URL
  spec.metadata["source_code_uri"] = GITHUB_URL
  spec.metadata["changelog_uri"] = "#{GITHUB_URL}/CHANGELOG.md"
  spec.metadata["bug_tracker_uri"] = "#{ROOT_URL}/issues"
  spec.metadata["documentation_uri"] = "#{GITHUB_URL}/README.md"

  spec.files = Dir["lib/**/*.rb"]
  spec.require_paths = ["lib"]

  spec.add_dependency "openfeature-sdk", "~> 0.4.1"
  spec.add_dependency "openssl", "~> 3.3" # Required for OpenSSL 3.6+ compatibility

  spec.add_development_dependency "rake", "~> 13.0"
  spec.add_development_dependency "rspec", "~> 3.12.0"
  spec.add_development_dependency "standard"
  spec.add_development_dependency "standard-performance"
  spec.add_development_dependency "simplecov", "~> 0.22.0"
  spec.add_development_dependency "simplecov-cobertura", "~> 2.1.0"
end
