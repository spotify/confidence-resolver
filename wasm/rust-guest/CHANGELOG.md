# Changelog

## [0.2.3](https://github.com/spotify/confidence-resolver/compare/rust-guest-v0.2.2...rust-guest-v0.2.3) (2026-05-27)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.17.1 to 0.18.0

## [0.2.2](https://github.com/spotify/confidence-resolver/compare/rust-guest-v0.2.1...rust-guest-v0.2.2) (2026-05-21)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.17.0 to 0.17.1

## [0.2.1](https://github.com/spotify/confidence-resolver/compare/rust-guest-v0.2.0...rust-guest-v0.2.1) (2026-05-13)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.16.1 to 0.17.0

## [0.2.0](https://github.com/spotify/confidence-resolver/compare/rust-guest-v0.1.23...rust-guest-v0.2.0) (2026-05-12)


### Features

* add register_resolve WASM export for SDK-side telemetry ([#354](https://github.com/spotify/confidence-resolver/issues/354)) ([31499bc](https://github.com/spotify/confidence-resolver/commit/31499bc911942ecd751b4a4a702f35ded41e9776))
* configurable gateway ([#236](https://github.com/spotify/confidence-resolver/issues/236)) ([aba6cbd](https://github.com/spotify/confidence-resolver/commit/aba6cbd65361db95a3b0391eee53e7c7e3cad717))
* error handling to avoid panics ([2a645a8](https://github.com/spotify/confidence-resolver/commit/2a645a87415bfce30af048498e068952b18ceb5e))
* Faster deploy image executions ([#10](https://github.com/spotify/confidence-resolver/issues/10)) ([d945331](https://github.com/spotify/confidence-resolver/commit/d9453317e9e40575e43d67558ef902a4bc62ee41))
* improve Prometheus metrics API ([#371](https://github.com/spotify/confidence-resolver/issues/371)) ([efb8c16](https://github.com/spotify/confidence-resolver/commit/efb8c16a4ca9659c60e4f69611c80d3addb4e3fa))
* local prometheus sdk metrics ([#322](https://github.com/spotify/confidence-resolver/issues/322)) ([8b18119](https://github.com/spotify/confidence-resolver/commit/8b18119eae449afbe4a0815e8aab5f82888a8621))
* metrics in telemetry data ([#266](https://github.com/spotify/confidence-resolver/issues/266)) ([965eff6](https://github.com/spotify/confidence-resolver/commit/965eff60b6c0377336e6e86624a78f6f3a859f73))
* Request per second in TelemetryData ([#150](https://github.com/spotify/confidence-resolver/issues/150)) ([b91669d](https://github.com/spotify/confidence-resolver/commit/b91669d75caa0971ab71d0589634ab039dae6081))
* Resolver WASM distribution via GitHub Releases  ([#12](https://github.com/spotify/confidence-resolver/issues/12)) ([23ab190](https://github.com/spotify/confidence-resolver/commit/23ab190016a923edfd5a82fe197929909ea0fc50))
* size limited flush api  ([#149](https://github.com/spotify/confidence-resolver/issues/149)) ([6ac60d6](https://github.com/spotify/confidence-resolver/commit/6ac60d6195421c9355941e4201993b521c831fcd))
* Support for startsWith and endsWith ([#283](https://github.com/spotify/confidence-resolver/issues/283)) ([661c4ef](https://github.com/spotify/confidence-resolver/commit/661c4ef53b5d907bf079e632135723c2af7af0b9))
* update sticky ([#38](https://github.com/spotify/confidence-resolver/issues/38)) ([41a42d2](https://github.com/spotify/confidence-resolver/commit/41a42d2917401de7389dcc37719b16de1e30199c))
* **wasm:** add wasm API to apply previously resolved flags ([#235](https://github.com/spotify/confidence-resolver/issues/235)) ([79048f6](https://github.com/spotify/confidence-resolver/commit/79048f63a8c771eb98ecf478cab0b654aa745374))


### Bug Fixes

* cap sampled schema count in resolve logging  ([#260](https://github.com/spotify/confidence-resolver/issues/260)) ([e84b724](https://github.com/spotify/confidence-resolver/commit/e84b7248f4b7ce9231902376ed56f3cb7b524f58))
* cleanup unused and fix lint warnings ([#61](https://github.com/spotify/confidence-resolver/issues/61)) ([1a85a78](https://github.com/spotify/confidence-resolver/commit/1a85a78e57232784bada3da692088d13f9b1089c))
* pass the account id together with the resolver state ([#30](https://github.com/spotify/confidence-resolver/issues/30)) ([20284b9](https://github.com/spotify/confidence-resolver/commit/20284b98097b0d7f794a1ed1e1243c9a8cdeff8f))
* pass the sdk with the state to get it into telemetry ([#332](https://github.com/spotify/confidence-resolver/issues/332)) ([1f22c5f](https://github.com/spotify/confidence-resolver/commit/1f22c5fa38c4e8a7d56ea4616db94f9b991d41ab))
* surface apply_flags errors instead of swallowing ([#386](https://github.com/spotify/confidence-resolver/issues/386)) ([7785f9f](https://github.com/spotify/confidence-resolver/commit/7785f9ff116c8151daa70b9fbaab9e66d8e88794))
* various memory issues ([#35](https://github.com/spotify/confidence-resolver/issues/35)) ([13c53fc](https://github.com/spotify/confidence-resolver/commit/13c53fcc5c1a51c90d51c47adb574316866c9b5b))
* wasm profile and small fixes ([#7](https://github.com/spotify/confidence-resolver/issues/7)) ([fae928b](https://github.com/spotify/confidence-resolver/commit/fae928b6c5d0923e4c82f2f4ae9b10bf0608beff))

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.16.0 to 0.16.1

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.15.1 to 0.16.0

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.15.0 to 0.15.1

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.14.0 to 0.15.0

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.13.0 to 0.14.0

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.12.1 to 0.13.0

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.12.0 to 0.12.1

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.11.1 to 0.12.0

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.11.0 to 0.11.1

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.10.0 to 0.11.0

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.9.0 to 0.10.0

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.8.0 to 0.9.0

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.7.0 to 0.8.0

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.6.0 to 0.7.0

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * wasm-msg bumped from 0.2.0 to 0.2.1
    * confidence_resolver bumped from 0.5.2 to 0.6.0

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.5.1 to 0.5.2

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.5.0 to 0.5.1

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * wasm-msg bumped from 0.1.0 to 0.2.0
    * confidence_resolver bumped from 0.4.0 to 0.5.0

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.3.1 to 0.4.0

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.3.0 to 0.3.1

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.2.0 to 0.3.0

## Changelog

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.1.0 to 0.2.0

## Changelog



## Changelog

This file is managed by Release Please.
