module github.com/spotify/confidence-resolver-rust/openfeature-provider/go/bench

go 1.24.0

require (
	github.com/open-feature/go-sdk v1.16.0
	github.com/spotify/confidence-resolver/openfeature-provider/go v0.0.0
	google.golang.org/grpc v1.79.3
)

require (
	github.com/go-logr/logr v1.4.3 // indirect
	github.com/tetratelabs/wazero v1.9.0 // indirect
	go.uber.org/mock v0.6.0 // indirect
	golang.org/x/net v0.48.0 // indirect
	golang.org/x/sys v0.39.0 // indirect
	golang.org/x/text v0.32.0 // indirect
	google.golang.org/genproto/googleapis/rpc v0.0.0-20251202230838-ff82c1b0f217 // indirect
	google.golang.org/protobuf v1.36.10 // indirect
)

replace github.com/spotify/confidence-resolver/openfeature-provider/go => ..
