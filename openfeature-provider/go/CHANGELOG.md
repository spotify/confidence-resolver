# Changelog

## [0.9.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/go/v0.8.1...openfeature-provider/go/v0.9.0) (2026-02-17)


### Features

* **go:** add configurable WASM resolver pool size ([#280](https://github.com/spotify/confidence-resolver/issues/280)) ([da054c7](https://github.com/spotify/confidence-resolver/commit/da054c7639afb378dd1b3b27653c176539a83442))

## [0.8.1](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/go/v0.8.0...openfeature-provider/go/v0.8.1) (2026-02-10)


### Bug Fixes

* cap sampled schema count in resolve logging  ([#260](https://github.com/spotify/confidence-resolver/issues/260)) ([e84b724](https://github.com/spotify/confidence-resolver/commit/e84b7248f4b7ce9231902376ed56f3cb7b524f58))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.14 to 0.1.15

## [0.8.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/go/v0.7.0...openfeature-provider/go/v0.8.0) (2026-01-28)


### Features

* **go:** support structs as default value ([#247](https://github.com/spotify/confidence-resolver/issues/247)) ([639d6c7](https://github.com/spotify/confidence-resolver/commit/639d6c781172e57d47a82b3d901d28e02365c436))

## [0.7.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/go/v0.6.0...openfeature-provider/go/v0.7.0) (2026-01-27)


### Features

* **go:** accept different intervals for state and logging ([#245](https://github.com/spotify/confidence-resolver/issues/245)) ([e1e64ec](https://github.com/spotify/confidence-resolver/commit/e1e64ecd6bd596afeb1f6f0c3ad3a603d3c8c0e5))
* **wasm:** add wasm API to apply previously resolved flags ([#235](https://github.com/spotify/confidence-resolver/issues/235)) ([79048f6](https://github.com/spotify/confidence-resolver/commit/79048f63a8c771eb98ecf478cab0b654aa745374))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.13 to 0.1.14

## [0.6.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/go/v0.5.0...openfeature-provider/go/v0.6.0) (2026-01-22)


### Features

* configurable gateway ([#236](https://github.com/spotify/confidence-resolver/issues/236)) ([aba6cbd](https://github.com/spotify/confidence-resolver/commit/aba6cbd65361db95a3b0391eee53e7c7e3cad717))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.12 to 0.1.13

## [0.5.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/go/v0.4.0...openfeature-provider/go/v0.5.0) (2025-12-19)


### Features

* add support for materialized segment targeting criteria ([#201](https://github.com/spotify/confidence-resolver/issues/201)) ([cdcfc86](https://github.com/spotify/confidence-resolver/commit/cdcfc8629400a268b3e3fe124ae0fbce8967182e))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.11 to 0.1.12

## [0.4.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/go/v0.3.0...openfeature-provider/go/v0.4.0) (2025-12-11)


### Features

* add remote materialization store for Go ([#203](https://github.com/spotify/confidence-resolver/issues/203)) ([d700af2](https://github.com/spotify/confidence-resolver/commit/d700af218eddc0db9b5f5edac1e211ce6fb1ffbf))
* add support for materialization stores ([#200](https://github.com/spotify/confidence-resolver/issues/200)) ([0f2ef79](https://github.com/spotify/confidence-resolver/commit/0f2ef79944b72e3ef06fc1bcc06848ff093402fc))


### Bug Fixes

* **go:** Fail hard on empty AccoundId ([#194](https://github.com/spotify/confidence-resolver/issues/194)) ([b5c90f7](https://github.com/spotify/confidence-resolver/commit/b5c90f7c0033f9e238331de725f5ce6dca77271f))
* **go:** Improve logs slightly ([#188](https://github.com/spotify/confidence-resolver/issues/188)) ([c013feb](https://github.com/spotify/confidence-resolver/commit/c013feb6086274b1c06d59cda85a8c5acf533609))

## [0.3.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/go/v0.2.0...openfeature-provider/go/v0.3.0) (2025-12-02)


### ⚠ BREAKING CHANGES

* customizable transport hooks ([#184](https://github.com/spotify/confidence-resolver/issues/184))
* migrate to cdn state fetch, publish logs using client secret ([#166](https://github.com/spotify/confidence-resolver/issues/166))

### Features

* customizable transport hooks ([#184](https://github.com/spotify/confidence-resolver/issues/184)) ([899b06d](https://github.com/spotify/confidence-resolver/commit/899b06dec64820d3662b1827769dc8779437d81a))
* migrate to cdn state fetch, publish logs using client secret ([#166](https://github.com/spotify/confidence-resolver/issues/166)) ([6c8d959](https://github.com/spotify/confidence-resolver/commit/6c8d959f124faa419c1ace103d8832457248eb26))


### Bug Fixes

* align the providers to do state fetching every 30 sec ([#180](https://github.com/spotify/confidence-resolver/issues/180)) ([6b537db](https://github.com/spotify/confidence-resolver/commit/6b537dbb51a587a7c09c3a285833a236cf5c51f9))
* update new test provider to include client secret ([#177](https://github.com/spotify/confidence-resolver/issues/177)) ([d281edf](https://github.com/spotify/confidence-resolver/commit/d281edf823235ddc9e2d86b0a25cf08f46d76a00))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.10 to 0.1.11

## [0.2.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/go/v0.1.0...openfeature-provider/go/v0.2.0) (2025-11-24)


### Features

* **openfeature-provider/go:** add slog-based logging ([#134](https://github.com/spotify/confidence-resolver/issues/134)) ([10362d9](https://github.com/spotify/confidence-resolver/commit/10362d9d6d80f25e63e12ba8c6257eb7e996a2c2))
* Request per second in TelemetryData ([#150](https://github.com/spotify/confidence-resolver/issues/150)) ([b91669d](https://github.com/spotify/confidence-resolver/commit/b91669d75caa0971ab71d0589634ab039dae6081))
* size limited flush api  ([#149](https://github.com/spotify/confidence-resolver/issues/149)) ([6ac60d6](https://github.com/spotify/confidence-resolver/commit/6ac60d6195421c9355941e4201993b521c831fcd))


### Bug Fixes

* **openfeature-provider/go:** move initialize work to provider.init ([#142](https://github.com/spotify/confidence-resolver/issues/142)) ([e1ef08a](https://github.com/spotify/confidence-resolver/commit/e1ef08a992fb980449ea267c7855eae396fe9e7e))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.9 to 0.1.10

## [0.1.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/go/v0.0.1...openfeature-provider/go/v0.1.0) (2025-11-11)


### ⚠ BREAKING CHANGES

* **go:** connection factory replaces server addr options ([#128](https://github.com/spotify/confidence-resolver/issues/128))
* **go:** correct module structure to match declared module path ([#106](https://github.com/spotify/confidence-resolver/issues/106))

### Features

* add go provider ([#100](https://github.com/spotify/confidence-resolver/issues/100)) ([5c0895b](https://github.com/spotify/confidence-resolver/commit/5c0895bd35edd7daf436be5a64b5a40ba3eb7dab))
* **go:** connection factory replaces server addr options ([#128](https://github.com/spotify/confidence-resolver/issues/128)) ([cd955a2](https://github.com/spotify/confidence-resolver/commit/cd955a22917c3572446cdc55491b1cd8b304763a))


### Bug Fixes

* **go:** Better error messaging for sticky rules ([#110](https://github.com/spotify/confidence-resolver/issues/110)) ([31a6893](https://github.com/spotify/confidence-resolver/commit/31a6893bb83c36abc2f8386912dcf316ee454e5a))
* **go:** correct module structure to match declared module path ([#106](https://github.com/spotify/confidence-resolver/issues/106)) ([c2eb597](https://github.com/spotify/confidence-resolver/commit/c2eb597d1c696bd1fac4459866b258ce852dbf9a))
* **go:** implement StateHandler for proper shutdown ([#109](https://github.com/spotify/confidence-resolver/issues/109)) ([6041e45](https://github.com/spotify/confidence-resolver/commit/6041e455c80ef24e9ac50c4881ce17ff40bee871))
* **openfeature/go:** fix openfeature reason mapping ([#121](https://github.com/spotify/confidence-resolver/issues/121)) ([c0334c5](https://github.com/spotify/confidence-resolver/commit/c0334c518af5eb294e8583b46e333864a1796507))

## Changelog
