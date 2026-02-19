# Changelog

## [0.12.0](https://github.com/spotify/confidence-resolver/compare/confidence_resolver-v0.11.1...confidence_resolver-v0.12.0) (2026-02-19)


### Features

* Support for startsWith and endsWith ([#283](https://github.com/spotify/confidence-resolver/issues/283)) ([661c4ef](https://github.com/spotify/confidence-resolver/commit/661c4ef53b5d907bf079e632135723c2af7af0b9))

## [0.11.1](https://github.com/spotify/confidence-resolver/compare/confidence_resolver-v0.11.0...confidence_resolver-v0.11.1) (2026-02-10)


### Bug Fixes

* cap sampled schema count in resolve logging  ([#260](https://github.com/spotify/confidence-resolver/issues/260)) ([e84b724](https://github.com/spotify/confidence-resolver/commit/e84b7248f4b7ce9231902376ed56f3cb7b524f58))

## [0.11.0](https://github.com/spotify/confidence-resolver/compare/confidence_resolver-v0.10.0...confidence_resolver-v0.11.0) (2026-01-27)


### Features

* **wasm:** add wasm API to apply previously resolved flags ([#235](https://github.com/spotify/confidence-resolver/issues/235)) ([79048f6](https://github.com/spotify/confidence-resolver/commit/79048f63a8c771eb98ecf478cab0b654aa745374))

## [0.10.0](https://github.com/spotify/confidence-resolver/compare/confidence_resolver-v0.9.0...confidence_resolver-v0.10.0) (2026-01-22)


### Features

* add Readmes and publishing steps ([#230](https://github.com/spotify/confidence-resolver/issues/230)) ([f40f9aa](https://github.com/spotify/confidence-resolver/commit/f40f9aa8b35c03bfb0e4fda7c68c4dab569581b4))

## [0.9.0](https://github.com/spotify/confidence-resolver/compare/confidence_resolver-v0.8.0...confidence_resolver-v0.9.0) (2025-12-19)


### Features

* add support for materialized segment targeting criteria ([#201](https://github.com/spotify/confidence-resolver/issues/201)) ([cdcfc86](https://github.com/spotify/confidence-resolver/commit/cdcfc8629400a268b3e3fe124ae0fbce8967182e))

## [0.8.0](https://github.com/spotify/confidence-resolver/compare/confidence_resolver-v0.7.0...confidence_resolver-v0.8.0) (2025-12-02)


### ⚠ BREAKING CHANGES

* migrate to cdn state fetch, publish logs using client secret ([#166](https://github.com/spotify/confidence-resolver/issues/166))

### Features

* migrate to cdn state fetch, publish logs using client secret ([#166](https://github.com/spotify/confidence-resolver/issues/166)) ([6c8d959](https://github.com/spotify/confidence-resolver/commit/6c8d959f124faa419c1ace103d8832457248eb26))

## [0.7.0](https://github.com/spotify/confidence-resolver/compare/confidence_resolver-v0.6.0...confidence_resolver-v0.7.0) (2025-11-24)


### Features

* Request per second in TelemetryData ([#150](https://github.com/spotify/confidence-resolver/issues/150)) ([b91669d](https://github.com/spotify/confidence-resolver/commit/b91669d75caa0971ab71d0589634ab039dae6081))
* send java sdk info in resolve request ([#160](https://github.com/spotify/confidence-resolver/issues/160)) ([8e10327](https://github.com/spotify/confidence-resolver/commit/8e103271886624187246ae86d8a78f74121a2f33))
* send js sdk info in resolve request ([#161](https://github.com/spotify/confidence-resolver/issues/161)) ([5cbc7d9](https://github.com/spotify/confidence-resolver/commit/5cbc7d9e2ada26c52298d74faaba50ec6cb4c3e7))
* size limited flush api  ([#149](https://github.com/spotify/confidence-resolver/issues/149)) ([6ac60d6](https://github.com/spotify/confidence-resolver/commit/6ac60d6195421c9355941e4201993b521c831fcd))

## [0.6.0](https://github.com/spotify/confidence-resolver-rust/compare/confidence_resolver-v0.5.2...confidence_resolver-v0.6.0) (2025-10-23)


### Features

* Add Java OpenFeature provider ([#58](https://github.com/spotify/confidence-resolver-rust/issues/58)) ([1bba814](https://github.com/spotify/confidence-resolver-rust/commit/1bba8145be547bce4f704585feef5f41d8dbc8bd))

## [0.5.2](https://github.com/spotify/confidence-resolver-rust/compare/confidence_resolver-v0.5.1...confidence_resolver-v0.5.2) (2025-10-14)


### Bug Fixes

* fix materialization default ([#59](https://github.com/spotify/confidence-resolver-rust/issues/59)) ([bdece6c](https://github.com/spotify/confidence-resolver-rust/commit/bdece6c5a4d53d6284056b6daaf9e8d17481db84))

## [0.5.1](https://github.com/spotify/confidence-resolver-rust/compare/confidence_resolver-v0.5.0...confidence_resolver-v0.5.1) (2025-10-14)


### Bug Fixes

* fix queue waiting ([#57](https://github.com/spotify/confidence-resolver-rust/issues/57)) ([b0da391](https://github.com/spotify/confidence-resolver-rust/commit/b0da3916f3179ab31ecea8196d106b303b5589d6))

## [0.5.0](https://github.com/spotify/confidence-resolver-rust/compare/confidence_resolver-v0.4.0...confidence_resolver-v0.5.0) (2025-10-10)


### Features

* js local resolver ([#32](https://github.com/spotify/confidence-resolver-rust/issues/32)) ([58893d6](https://github.com/spotify/confidence-resolver-rust/commit/58893d6610b56b5aa6a6250db9e9bb1af506497f))


### Bug Fixes

* handle arithmetic panics ([#45](https://github.com/spotify/confidence-resolver-rust/issues/45)) ([83912c9](https://github.com/spotify/confidence-resolver-rust/commit/83912c9d93665b01a4cc1e9cd8e520fd94f2e21a))
* handle arithmetic panics ([#45](https://github.com/spotify/confidence-resolver-rust/issues/45)) ([6713c28](https://github.com/spotify/confidence-resolver-rust/commit/6713c28196b351db9ba22eb87ca7b83ef4a25ff3))
* return false out of range ([#47](https://github.com/spotify/confidence-resolver-rust/issues/47)) ([0a91568](https://github.com/spotify/confidence-resolver-rust/commit/0a915688d115df760692879c5e93f62c54a09d56))

## [0.4.0](https://github.com/spotify/confidence-resolver-rust/compare/confidence_resolver-v0.3.1...confidence_resolver-v0.4.0) (2025-10-03)


### Features

* update sticky ([#38](https://github.com/spotify/confidence-resolver-rust/issues/38)) ([41a42d2](https://github.com/spotify/confidence-resolver-rust/commit/41a42d2917401de7389dcc37719b16de1e30199c))

## [0.3.1](https://github.com/spotify/confidence-resolver-rust/compare/confidence_resolver-v0.3.0...confidence_resolver-v0.3.1) (2025-10-02)


### Bug Fixes

* various memory issues ([#35](https://github.com/spotify/confidence-resolver-rust/issues/35)) ([13c53fc](https://github.com/spotify/confidence-resolver-rust/commit/13c53fcc5c1a51c90d51c47adb574316866c9b5b))

## [0.3.0](https://github.com/spotify/confidence-resolver-rust/compare/confidence_resolver-v0.2.0...confidence_resolver-v0.3.0) (2025-09-08)


### Features

* error handling to avoid panics ([2a645a8](https://github.com/spotify/confidence-resolver-rust/commit/2a645a87415bfce30af048498e068952b18ceb5e))

## [0.2.0](https://github.com/spotify/confidence-resolver-rust/compare/confidence_resolver-v0.1.0...confidence_resolver-v0.2.0) (2025-09-05)


### Features

* Support non-fractional Integers as targeting key ([#17](https://github.com/spotify/confidence-resolver-rust/issues/17)) ([554f442](https://github.com/spotify/confidence-resolver-rust/commit/554f442551e3ef8213a52cca266054ae5da0511e))

## 0.1.0 (2025-09-03)


### Features

* Faster deploy image executions ([#10](https://github.com/spotify/confidence-resolver-rust/issues/10)) ([d945331](https://github.com/spotify/confidence-resolver-rust/commit/d9453317e9e40575e43d67558ef902a4bc62ee41))


### Bug Fixes

* wasm profile and small fixes ([#7](https://github.com/spotify/confidence-resolver-rust/issues/7)) ([fae928b](https://github.com/spotify/confidence-resolver-rust/commit/fae928b6c5d0923e4c82f2f4ae9b10bf0608beff))

## Changelog

This file is managed by Release Please.
