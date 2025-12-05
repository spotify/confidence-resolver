package confidence

import (
	"context"
)

// UnsupportedMaterializationStore is a MaterializationStore implementation that always
// returns MaterializationNotSupportedError. This is the default store used by the provider
// to trigger fallback to remote gRPC resolution when materializations are needed.
//
// This allows the Confidence service to manage materializations server-side,
// requiring no additional client-side setup.
type UnsupportedMaterializationStore struct{}

// NewUnsupportedMaterializationStore creates a new UnsupportedMaterializationStore.
func NewUnsupportedMaterializationStore() *UnsupportedMaterializationStore {
	return &UnsupportedMaterializationStore{}
}

// Read always returns MaterializationNotSupportedError to trigger gRPC fallback.
func (u *UnsupportedMaterializationStore) Read(ctx context.Context, ops []ReadOp) ([]ReadResult, error) {
	return nil, &MaterializationNotSupportedError{
		Message: "materialization read not supported, falling back to remote resolution",
	}
}

// Write always returns MaterializationNotSupportedError.
func (u *UnsupportedMaterializationStore) Write(ctx context.Context, ops []WriteOp) error {
	return &MaterializationNotSupportedError{
		Message: "materialization write not supported",
	}
}

// Close is a no-op for UnsupportedMaterializationStore.
func (u *UnsupportedMaterializationStore) Close() error {
	return nil
}
