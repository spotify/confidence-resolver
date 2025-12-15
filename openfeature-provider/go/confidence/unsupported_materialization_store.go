package confidence

import (
	"context"
)

// unsupportedMaterializationStore is a MaterializationStore implementation that always
// returns MaterializationNotSupportedError. This is the default store used by the provider
// to trigger fallback to remote gRPC resolution when materializations are needed.
//
// This allows the Confidence service to manage materializations server-side,
// requiring no additional client-side setup.
type unsupportedMaterializationStore struct{}

// newUnsupportedMaterializationStore creates a new UnsupportedMaterializationStore.
func newUnsupportedMaterializationStore() *unsupportedMaterializationStore {
	return &unsupportedMaterializationStore{}
}

// Read always returns MaterializationNotSupportedError to trigger gRPC fallback.
func (u *unsupportedMaterializationStore) Read(ctx context.Context, ops []ReadOp) ([]ReadResult, error) {
	return nil, &MaterializationNotSupportedError{
		Message: "materialization read not supported",
	}
}

// Write always returns MaterializationNotSupportedError.
func (u *unsupportedMaterializationStore) Write(ctx context.Context, ops []WriteOp) error {
	return &MaterializationNotSupportedError{
		Message: "materialization write not supported",
	}
}
