# Changelog

## [0.11.1](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-java-v0.11.0...openfeature-provider-java-v0.11.1) (2026-01-22)


### Bug Fixes

* **java:** handle CDN failures gracefully during initialization ([#222](https://github.com/spotify/confidence-resolver/issues/222)) ([841354d](https://github.com/spotify/confidence-resolver/commit/841354d64396a2d36ed6a9ac0f620ab9d07690d7))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.12 to 0.1.13

## [0.11.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-java-v0.10.1...openfeature-provider-java-v0.11.0) (2026-01-09)


### ⚠ BREAKING CHANGES

* **java:** move provider to com.spotify.confidence.sdk package ([#215](https://github.com/spotify/confidence-resolver/issues/215))

### Features

* **java:** shade gRPC/protobuf to avoid version conflicts ([#217](https://github.com/spotify/confidence-resolver/issues/217)) ([7abdec8](https://github.com/spotify/confidence-resolver/commit/7abdec85ae26e9642850501579338ac4a4255a26))


### Code Refactoring

* **java:** move provider to com.spotify.confidence.sdk package ([#215](https://github.com/spotify/confidence-resolver/issues/215)) ([d4c53f2](https://github.com/spotify/confidence-resolver/commit/d4c53f26ebbe21de71ebdc37cf03dec803aea7bc))

## [0.10.1](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-java-v0.10.0...openfeature-provider-java-v0.10.1) (2025-12-19)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.11 to 0.1.12

## [0.10.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-java-v0.9.1...openfeature-provider-java-v0.10.0) (2025-12-11)


### Features

* WARN logs for errors in evaluations ([#192](https://github.com/spotify/confidence-resolver/issues/192)) ([7a9f157](https://github.com/spotify/confidence-resolver/commit/7a9f1571639e24ea366e643b5b966b1de6fe06bf))


### Bug Fixes

* java shutdown sequence fixing ([#167](https://github.com/spotify/confidence-resolver/issues/167)) ([17dfa47](https://github.com/spotify/confidence-resolver/commit/17dfa476d39c974053ea888e0d75d39be4f4e12b))

## [0.9.1](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-java-v0.9.0...openfeature-provider-java-v0.9.1) (2025-12-03)


### Bug Fixes

* formatting ([#189](https://github.com/spotify/confidence-resolver/issues/189)) ([b88849a](https://github.com/spotify/confidence-resolver/commit/b88849aa804f80ab014e6b94ef84569556efa7a3))

## [0.9.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-java-v0.8.0...openfeature-provider-java-v0.9.0) (2025-12-02)


### ⚠ BREAKING CHANGES

* java materialization interface rework ([#170](https://github.com/spotify/confidence-resolver/issues/170))
* migrate to cdn state fetch, publish logs using client secret ([#166](https://github.com/spotify/confidence-resolver/issues/166))

### Features

* migrate to cdn state fetch, publish logs using client secret ([#166](https://github.com/spotify/confidence-resolver/issues/166)) ([6c8d959](https://github.com/spotify/confidence-resolver/commit/6c8d959f124faa419c1ace103d8832457248eb26))
* set the nr of wasm instances with an env var ([#158](https://github.com/spotify/confidence-resolver/issues/158)) ([8ba8900](https://github.com/spotify/confidence-resolver/commit/8ba8900e5717931a3132a2f916889ef6a74a80a7))


### Bug Fixes

* align the providers to do state fetching every 30 sec ([#180](https://github.com/spotify/confidence-resolver/issues/180)) ([6b537db](https://github.com/spotify/confidence-resolver/commit/6b537dbb51a587a7c09c3a285833a236cf5c51f9))


### Code Refactoring

* java materialization interface rework ([#170](https://github.com/spotify/confidence-resolver/issues/170)) ([fa6955a](https://github.com/spotify/confidence-resolver/commit/fa6955a94b41be8fc0292c9c8bbf76aac6bcd852))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.10 to 0.1.11

## [0.8.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-java-v0.7.4...openfeature-provider-java-v0.8.0) (2025-11-24)


### Features

* **openfeature-provider/java:** connectionfactory for testing ([#147](https://github.com/spotify/confidence-resolver/issues/147)) ([e1ca77e](https://github.com/spotify/confidence-resolver/commit/e1ca77efc26cbd8cfc6f822e691b385328bf8f53))
* **openfeature-provider/java:** make java provider init as in the OF spec ([#151](https://github.com/spotify/confidence-resolver/issues/151)) ([1adf48e](https://github.com/spotify/confidence-resolver/commit/1adf48eea2c70ad94d85c8e803e5c81ab439c02b))
* Request per second in TelemetryData ([#150](https://github.com/spotify/confidence-resolver/issues/150)) ([b91669d](https://github.com/spotify/confidence-resolver/commit/b91669d75caa0971ab71d0589634ab039dae6081))
* send java sdk info in resolve request ([#160](https://github.com/spotify/confidence-resolver/issues/160)) ([8e10327](https://github.com/spotify/confidence-resolver/commit/8e103271886624187246ae86d8a78f74121a2f33))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.9 to 0.1.10

## [0.7.4](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-java-v0.7.3...openfeature-provider-java-v0.7.4) (2025-11-11)


### Bug Fixes

* **openfeature/java:** update readme and fix release please update ([#120](https://github.com/spotify/confidence-resolver/issues/120)) ([7e78391](https://github.com/spotify/confidence-resolver/commit/7e7839143ba7f77007bac554006bc36dade172a3))

## [0.7.3](https://github.com/spotify/confidence-resolver/compare/openfeature-provider-java-v0.7.2...openfeature-provider-java-v0.7.3) (2025-11-07)


### Bug Fixes

* **java:** reload state before creating the initial Resolver ([#104](https://github.com/spotify/confidence-resolver/issues/104)) ([93581bd](https://github.com/spotify/confidence-resolver/commit/93581bd65f5775b9f188a1c9962153428cc76bdc))

## [0.7.2](https://github.com/spotify/confidence-resolver-rust/compare/openfeature-provider-java-v0.7.1...openfeature-provider-java-v0.7.2) (2025-11-03)


### Bug Fixes

* **deps-dev:** bump commons-lang3 from 3.17.0 to 3.18.0 in /openfeature-provider/java ([#89](https://github.com/spotify/confidence-resolver-rust/issues/89)) ([7db65a6](https://github.com/spotify/confidence-resolver-rust/commit/7db65a662374d2aab01c77f243082c482113981e))

## [0.7.1](https://github.com/spotify/confidence-resolver-rust/compare/openfeature-provider-java-v0.7.0...openfeature-provider-java-v0.7.1) (2025-10-23)


### Bug Fixes

* java refactoring and cleanups ([#74](https://github.com/spotify/confidence-resolver-rust/issues/74)) ([700881e](https://github.com/spotify/confidence-resolver-rust/commit/700881ef9605e950607a40984664decf33dc8643))

## [0.7.0](https://github.com/spotify/confidence-resolver-rust/compare/openfeature-provider-java-v0.6.4...openfeature-provider-java-v0.7.0) (2025-10-23)


### Features

* [release-please] Java Provider support ([#68](https://github.com/spotify/confidence-resolver-rust/issues/68)) ([9478533](https://github.com/spotify/confidence-resolver-rust/commit/9478533960bf02e86d4ed1aab7ac1edd5034c3fb))
* Add Java OpenFeature provider ([#58](https://github.com/spotify/confidence-resolver-rust/issues/58)) ([1bba814](https://github.com/spotify/confidence-resolver-rust/commit/1bba8145be547bce4f704585feef5f41d8dbc8bd))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rust-guest bumped from 0.1.8 to 0.1.9

## 0.6.4 (2025-10-20)

This release was not made from this repository but is mentioned here for linking sake. The release was made from the deprecated repository previously used to work on this provider ( reference: https://github.com/spotify/confidence-sdk-java/releases/tag/v0.6.4).
