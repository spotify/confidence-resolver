# openfeature-provider/ruby Makefile
# Ruby OpenFeature provider

ROOT := $(realpath $(CURDIR)/../..)

# Stamps and inputs
INSTALL_STAMP := .install.stamp
BUILD_STAMP := .build.stamp
SRC := $(shell find lib -name '*.rb' 2>/dev/null)
SPEC := $(shell find spec -name '*.rb' 2>/dev/null)
GEMSPEC := confidence-openfeature-provider.gemspec

.PHONY: install build test lint clean

# Install dependencies only when Gemfile changes
$(INSTALL_STAMP): Gemfile
	bundle install
	@touch $@

install: $(INSTALL_STAMP)

# Build gem package
$(BUILD_STAMP): $(GEMSPEC) $(INSTALL_STAMP) $(SRC)
	bundle exec rake build
	@touch $@

build: $(BUILD_STAMP)

# Run tests
test: $(INSTALL_STAMP)
	bundle exec rake spec

# Run linter (standard)
lint: $(INSTALL_STAMP)
	bundle exec rake standard

# Clean build artifacts
clean:
	rm -rf pkg/
	rm -f $(INSTALL_STAMP) $(BUILD_STAMP)
	rm -rf coverage/
	rm -f .rspec_status

.DEFAULT_GOAL := build
