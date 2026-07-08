# Changelog

## [0.7.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.6.1...openfeature-provider/rust-v0.7.0) (2026-07-08)


### Features

* **rust:** support encrypted CDN resolver state ([#461](https://github.com/spotify/confidence-resolver/issues/461)) ([abdae52](https://github.com/spotify/confidence-resolver/commit/abdae52ded4fae9d93d6ef3d0cc0318d7de82d79))


### Bug Fixes

* **rust:** return None when path traverses through non-struct leaf ([#467](https://github.com/spotify/confidence-resolver/issues/467)) ([2a955e0](https://github.com/spotify/confidence-resolver/commit/2a955e0e9c6ec427aede25994fe9f932b27847a3))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.19.0 to 0.19.1

## [0.6.1](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.6.0...openfeature-provider/rust-v0.6.1) (2026-06-17)


### Bug Fixes

* **rust:** return null-specific error instead of TypeMismatch for unset properties ([#425](https://github.com/spotify/confidence-resolver/issues/425)) ([95144b6](https://github.com/spotify/confidence-resolver/commit/95144b63840e9903e415d6a926061439b0d90b46))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.18.1 to 0.19.0

## [0.6.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.5.6...openfeature-provider/rust-v0.6.0) (2026-05-28)


### Features

* support skipping apply using context key ([#421](https://github.com/spotify/confidence-resolver/issues/421)) ([0ad63cc](https://github.com/spotify/confidence-resolver/commit/0ad63ccebef53073e57be42a9ab78a03f14e1851))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.18.0 to 0.18.1

## [0.5.6](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.5.5...openfeature-provider/rust-v0.5.6) (2026-05-27)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.17.1 to 0.18.0

## [0.5.5](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.5.4...openfeature-provider/rust-v0.5.5) (2026-05-21)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.17.0 to 0.17.1

## [0.5.4](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.5.3...openfeature-provider/rust-v0.5.4) (2026-05-13)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.16.1 to 0.17.0

## [0.5.3](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.5.2...openfeature-provider/rust-v0.5.3) (2026-04-28)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.16.0 to 0.16.1

## [0.5.2](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.5.1...openfeature-provider/rust-v0.5.2) (2026-04-15)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.15.1 to 0.16.0

## [0.5.1](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.5.0...openfeature-provider/rust-v0.5.1) (2026-03-26)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.15.0 to 0.15.1

## [0.5.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.4.1...openfeature-provider/rust-v0.5.0) (2026-03-24)


### Features

* add register_resolve WASM export for SDK-side telemetry ([#354](https://github.com/spotify/confidence-resolver/issues/354)) ([31499bc](https://github.com/spotify/confidence-resolver/commit/31499bc911942ecd751b4a4a702f35ded41e9776))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.14.0 to 0.15.0

## [0.4.1](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.4.0...openfeature-provider/rust-v0.4.1) (2026-03-12)


### Bug Fixes

* pass the sdk with the state to get it into telemetry ([#332](https://github.com/spotify/confidence-resolver/issues/332)) ([1f22c5f](https://github.com/spotify/confidence-resolver/commit/1f22c5fa38c4e8a7d56ea4616db94f9b991d41ab))
* **rust:** include proto files in cargo publish tarball ([#318](https://github.com/spotify/confidence-resolver/issues/318)) ([437502d](https://github.com/spotify/confidence-resolver/commit/437502d02e06f27413a5270b923ac2db032de7c6))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.13.0 to 0.14.0

## [0.4.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.3.1...openfeature-provider/rust-v0.4.0) (2026-03-06)


### Features

* metrics in telemetry data ([#266](https://github.com/spotify/confidence-resolver/issues/266)) ([965eff6](https://github.com/spotify/confidence-resolver/commit/965eff60b6c0377336e6e86624a78f6f3a859f73))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.12.1 to 0.13.0

## [0.3.1](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.3.0...openfeature-provider/rust-v0.3.1) (2026-02-20)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.12.0 to 0.12.1

## [0.3.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.2.2...openfeature-provider/rust-v0.3.0) (2026-02-19)


### Features

* Support for startsWith and endsWith ([#283](https://github.com/spotify/confidence-resolver/issues/283)) ([661c4ef](https://github.com/spotify/confidence-resolver/commit/661c4ef53b5d907bf079e632135723c2af7af0b9))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.11.1 to 0.12.0

## [0.2.2](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.2.1...openfeature-provider/rust-v0.2.2) (2026-02-10)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.11.0 to 0.11.1

## [0.2.1](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.2.0...openfeature-provider/rust-v0.2.1) (2026-01-27)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.10.0 to 0.11.0

## [0.2.0](https://github.com/spotify/confidence-resolver/compare/openfeature-provider/rust-v0.1.0...openfeature-provider/rust-v0.2.0) (2026-01-22)


### Features

* add Readmes and publishing steps ([#230](https://github.com/spotify/confidence-resolver/issues/230)) ([f40f9aa](https://github.com/spotify/confidence-resolver/commit/f40f9aa8b35c03bfb0e4fda7c68c4dab569581b4))
* add rust provider ([#223](https://github.com/spotify/confidence-resolver/issues/223)) ([9eefb80](https://github.com/spotify/confidence-resolver/commit/9eefb80bbca5272019c41801bd1387955adc468d))
* configurable gateway ([#236](https://github.com/spotify/confidence-resolver/issues/236)) ([aba6cbd](https://github.com/spotify/confidence-resolver/commit/aba6cbd65361db95a3b0391eee53e7c7e3cad717))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * confidence_resolver bumped from 0.9.0 to 0.10.0
