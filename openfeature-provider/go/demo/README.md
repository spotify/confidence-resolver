# Confidence OpenFeature Provider Demo

Demo application showing how to use the Confidence OpenFeature Local Provider in Go.

## Prerequisites

- Go 1.24+
- Confidence API credentials

## Setup

Set the required environment variable:

```bash
export CONFIDENCE_CLIENT_SECRET="your-client-secret"
```

Get your credentials from the [Confidence dashboard](https://confidence.spotify.com/).

## Run

```bash
go run main.go
```

The demo runs concurrent flag evaluations to test performance and state synchronization.

## Configuration

The provider supports custom poll intervals for state updates and log flushing:

```go
provider, err := confidence.NewProvider(ctx, confidence.ProviderConfig{
    ClientSecret:                  clientSecret,
    UseRemoteMaterializationStore: true,
    StatePollInterval:             30 * time.Second,  // How often to fetch flag state (default: 10s)
    LogPollInterval:               2 * time.Minute,   // How often to flush logs (default: 60s)
})
```

| Option | Default | Description |
|--------|---------|-------------|
| `StatePollInterval` | 10s | Interval for polling flag state updates |
| `LogPollInterval` | 60s | Interval for flushing evaluation logs |
| `ResolverPoolSize` | GOMAXPROCS | Number of WASM resolver instances |
