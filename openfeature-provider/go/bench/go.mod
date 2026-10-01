module github.com/spotify/confidence-resolver-rust/openfeature-provider/go/bench

go 1.26.0

require (
	github.com/open-feature/go-sdk v1.19.0
	github.com/spotify/confidence-resolver/openfeature-provider/go v0.0.0
	google.golang.org/grpc v1.84.0
)

require (
	github.com/tetratelabs/wazero v1.12.0 // indirect
	go.uber.org/mock v0.6.0 // indirect
	golang.org/x/net v0.59.0 // indirect
	golang.org/x/sys v0.48.0 // indirect
	golang.org/x/text v0.42.0 // indirect
	google.golang.org/genproto/googleapis/rpc v0.0.0-20260706201446-f0a921348800 // indirect
	google.golang.org/protobuf v1.36.12 // indirect
)

replace github.com/spotify/confidence-resolver/openfeature-provider/go => ..
