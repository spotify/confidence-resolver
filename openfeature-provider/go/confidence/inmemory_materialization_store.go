package confidence

import (
	"context"
	"log/slog"
	"sync"
)

// inMemoryMaterializationStore is a thread-safe in-memory implementation of MaterializationStore.
//
// ⚠️ For Testing/Example Only: This implementation is suitable for testing and as a reference
// but should NOT be used in production because:
//   - Data is lost on application restart (no persistence)
//   - No TTL management (entries never expire)
//   - Memory grows unbounded
//   - Not suitable for multi-instance deployments
//
// Thread Safety: This implementation is thread-safe using sync.RWMutex for concurrent access.
//
// Storage Structure:
//
//	unit → materialization → MaterializationData
//	  where MaterializationData contains:
//	    - included: bool (whether unit is in materialized segment)
//	    - variants: map[rule]variant (sticky variant assignments)
//
// Production Implementation: For production use, implement MaterializationStore with
// persistent storage like Redis, DynamoDB, etc.
type inMemoryMaterializationStore struct {
	// storage: unit -> materialization -> data
	storage map[string]map[string]*materializationData
	mu      sync.RWMutex
	logger  *slog.Logger
	// call tracking for tests
	readCalls  [][]ReadOp
	writeCalls [][]WriteOp
}

type materializationData struct {
	included      bool              // whether unit is in materialized segment
	ruleToVariant map[string]string // rule -> variant mappings for sticky assignments
}

// newInMemoryMaterializationStore creates a new in-memory materialization store.
func newInMemoryMaterializationStore(logger *slog.Logger) *inMemoryMaterializationStore {
	if logger == nil {
		logger = slog.Default()
	}
	return &inMemoryMaterializationStore{
		storage: make(map[string]map[string]*materializationData),
		logger:  logger,
	}
}

// newInMemoryMaterializationStoreWithInclusions creates a store pre-populated with inclusion data.
// This is useful for testing materialized segment criterion evaluation.
// The initialInclusions map structure is: unit -> materialization -> included
func newInMemoryMaterializationStoreWithInclusions(logger *slog.Logger, initialInclusions map[string]map[string]bool) *inMemoryMaterializationStore {
	store := newInMemoryMaterializationStore(logger)

	store.mu.Lock()
	defer store.mu.Unlock()

	for unit, materializations := range initialInclusions {
		if store.storage[unit] == nil {
			store.storage[unit] = make(map[string]*materializationData)
		}
		for materialization, included := range materializations {
			store.storage[unit][materialization] = &materializationData{
				included:      included,
				ruleToVariant: make(map[string]string),
			}
		}
	}

	return store
}

// Read performs a batch read of materialization data.
func (s *inMemoryMaterializationStore) Read(ctx context.Context, ops []ReadOp) ([]ReadResult, error) {
	s.mu.Lock()
	// track call
	// make a shallow copy to avoid external mutation
	copied := make([]ReadOp, len(ops))
	copy(copied, ops)
	s.readCalls = append(s.readCalls, copied)
	s.mu.Unlock()

	s.mu.RLock()
	defer s.mu.RUnlock()

	results := make([]ReadResult, len(ops))
	for i, op := range ops {
		switch readOp := op.(type) {
		case *ReadOpInclusion:
			included := false
			if unitData, ok := s.storage[readOp.Unit()]; ok {
				if data, ok := unitData[readOp.Materialization()]; ok {
					included = data.included
				}
			}
			results[i] = readOp.ToResult(included)
			s.logger.Debug("Read inclusion",
				"unit", readOp.Unit(),
				"materialization", readOp.Materialization(),
				"result", included)

		case *ReadOpVariant:
			var variant *string
			if unitData, ok := s.storage[readOp.Unit()]; ok {
				if data, ok := unitData[readOp.Materialization()]; ok {
					if v, ok := data.ruleToVariant[readOp.Rule()]; ok {
						variant = &v
					}
				}
			}
			results[i] = readOp.ToResult(variant)
			s.logger.Debug("Read variant",
				"unit", readOp.Unit(),
				"materialization", readOp.Materialization(),
				"rule", readOp.Rule(),
				"result", variant)
		}
	}

	return results, nil
}

// Write performs a batch write of materialization data.
func (s *inMemoryMaterializationStore) Write(ctx context.Context, ops []WriteOp) error {
	s.mu.Lock()
	// track call
	copied := make([]WriteOp, len(ops))
	copy(copied, ops)
	s.writeCalls = append(s.writeCalls, copied)
	defer s.mu.Unlock()

	for _, op := range ops {
		switch writeOp := op.(type) {
		case *WriteOpVariant:
			unit := writeOp.Unit()
			mat := writeOp.Materialization()

			// Ensure unit exists
			if s.storage[unit] == nil {
				s.storage[unit] = make(map[string]*materializationData)
			}

			// Ensure materialization exists
			if s.storage[unit][mat] == nil {
				s.storage[unit][mat] = &materializationData{
					included:      false,
					ruleToVariant: make(map[string]string),
				}
			}

			// Store the variant and mark as included
			s.storage[unit][mat].ruleToVariant[writeOp.Rule()] = writeOp.Variant()
			s.storage[unit][mat].included = true

			s.logger.Debug("Wrote variant",
				"unit", unit,
				"materialization", mat,
				"rule", writeOp.Rule(),
				"variant", writeOp.Variant())
		}
	}

	return nil
}

// Close clears all stored materialization data from memory.
// Call this method during application shutdown or test cleanup to free memory.
func (s *inMemoryMaterializationStore) Close() error {
	s.mu.Lock()
	defer s.mu.Unlock()

	s.storage = make(map[string]map[string]*materializationData)
	s.readCalls = nil
	s.writeCalls = nil
	s.logger.Debug("In-memory storage cleared")
	return nil
}

// ReadCalls returns a snapshot of all read calls and their ops in chronological order.
func (s *inMemoryMaterializationStore) ReadCalls() [][]ReadOp {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([][]ReadOp, len(s.readCalls))
	for i, call := range s.readCalls {
		copied := make([]ReadOp, len(call))
		copy(copied, call)
		out[i] = copied
	}
	return out
}

// WriteCalls returns a snapshot of all write calls and their ops in chronological order.
func (s *inMemoryMaterializationStore) WriteCalls() [][]WriteOp {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([][]WriteOp, len(s.writeCalls))
	for i, call := range s.writeCalls {
		copied := make([]WriteOp, len(call))
		copy(copied, call)
		out[i] = copied
	}
	return out
}
