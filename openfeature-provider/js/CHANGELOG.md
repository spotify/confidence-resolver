# Changelog

## [0.8.1](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-js-v0.8.0...openfeature-provider-js-v0.8.1) (2026-02-10)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.14 to 0.1.15

## [0.8.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-js-v0.7.0...openfeature-provider-js-v0.8.0) (2026-01-27)


### Features

* **js:** add React support with useFlag and useFlagDetails hooks ([#246](https://github.com/spotify/confidence-resolver/issues/246)) ([d579a4c](https://github.com/spotify/confidence-resolver/commit/d579a4c8fe493ee3a92539203f21d1c03758c58e))
* **wasm:** add wasm API to apply previously resolved flags ([#235](https://github.com/spotify/confidence-resolver/issues/235)) ([79048f6](https://github.com/spotify/confidence-resolver/commit/79048f63a8c771eb98ecf478cab0b654aa745374))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.13 to 0.1.14

## [0.7.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-js-v0.6.0...openfeature-provider-js-v0.7.0) (2026-01-22)


### Features

* inlined WASM for improved portability ([#243](https://github.com/spotify/confidence-resolver/issues/243)) ([8d86283](https://github.com/spotify/confidence-resolver/commit/8d862837244ffd9099bfeb56b42e203212e73a96))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.12 to 0.1.13

## [0.6.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-js-v0.5.1...openfeature-provider-js-v0.6.0) (2026-01-15)


### Features

* **js:** add stateUpdateInterval option for configurable state polling ([#224](https://github.com/spotify/confidence-resolver/issues/224)) ([5687847](https://github.com/spotify/confidence-resolver/commit/56878478d3d508870ac0f3172f897c8728e8a85d))


### Bug Fixes

* prevent split bundle ([#212](https://github.com/spotify/confidence-resolver/issues/212)) ([80cfd0d](https://github.com/spotify/confidence-resolver/commit/80cfd0d0128af4c65b1ae50539f2b1d0d959c02e))

## [0.5.1](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-js-v0.5.0...openfeature-provider-js-v0.5.1) (2026-01-12)


### Bug Fixes

* **js:** handle missing debug package under nextjs ([#219](https://github.com/spotify/confidence-resolver/issues/219)) ([c4d49fc](https://github.com/spotify/confidence-resolver/commit/c4d49fc6486931e902848f11245c2a4d02af98ff))

## [0.5.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-js-v0.4.0...openfeature-provider-js-v0.5.0) (2025-12-19)


### Features

* pluggable materialization ([#211](https://github.com/spotify/confidence-resolver/issues/211)) ([96aea72](https://github.com/spotify/confidence-resolver/commit/96aea72a2dce2e02fdfa4374e4fa245f0efc177b))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.11 to 0.1.12

## [0.4.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-js-v0.3.0...openfeature-provider-js-v0.4.0) (2025-12-11)


### Features

* **js:** flush assigned when reaching limit ([#145](https://github.com/spotify/confidence-resolver/issues/145)) ([09293ad](https://github.com/spotify/confidence-resolver/commit/09293adc5800f477620905f28539ad45d6a40997))
* WARN logs for errors in evaluations ([#192](https://github.com/spotify/confidence-resolver/issues/192)) ([7a9f157](https://github.com/spotify/confidence-resolver/commit/7a9f1571639e24ea366e643b5b966b1de6fe06bf))

## [0.3.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-js-v0.2.0...openfeature-provider-js-v0.3.0) (2025-12-02)


### ⚠ BREAKING CHANGES

* migrate to cdn state fetch, publish logs using client secret ([#166](https://github.com/spotify/confidence-resolver/issues/166))

### Features

* migrate to cdn state fetch, publish logs using client secret ([#166](https://github.com/spotify/confidence-resolver/issues/166)) ([6c8d959](https://github.com/spotify/confidence-resolver/commit/6c8d959f124faa419c1ace103d8832457248eb26))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.10 to 0.1.11

## [0.2.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-js-v0.1.1...openfeature-provider-js-v0.2.0) (2025-11-24)


### Features

* send js sdk info in resolve request ([#161](https://github.com/spotify/confidence-resolver/issues/161)) ([5cbc7d9](https://github.com/spotify/confidence-resolver/commit/5cbc7d9e2ada26c52298d74faaba50ec6cb4c3e7))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.9 to 0.1.10

## [0.1.1](https://github.com/spotify/confidence-resolver-rust/compare/openfeature-provider-js-v0.1.0...openfeature-provider-js-v0.1.1) (2025-11-03)


### Bug Fixes

* handle panics ([#76](https://github.com/spotify/confidence-resolver-rust/issues/76)) ([1ea86ea](https://github.com/spotify/confidence-resolver-rust/commit/1ea86eaa3e64aea5a64086534fe94a155828ef80))

## 0.1.0 (2025-10-24)


### Features

* js local resolver ([#32](https://github.com/spotify/confidence-resolver-rust/issues/32)) ([58893d6](https://github.com/spotify/confidence-resolver-rust/commit/58893d6610b56b5aa6a6250db9e9bb1af506497f))
* **js:** add sticky assignment support through remote resolver fallback ([#48](https://github.com/spotify/confidence-resolver-rust/issues/48)) ([baeff3e](https://github.com/spotify/confidence-resolver-rust/commit/baeff3ec39a084615012d961997949748285ac57))


### Bug Fixes

* **js:** correct current time ([#67](https://github.com/spotify/confidence-resolver-rust/issues/67)) ([260f5e3](https://github.com/spotify/confidence-resolver-rust/commit/260f5e3c937dad99e09db1795c6036f0647514c7))

## 0.0.0 (Initial Version)

* Initial setup for release-please tracking
