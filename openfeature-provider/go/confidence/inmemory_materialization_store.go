package confidence

import (
	"context"
	"log/slog"
	"sync"
)

// InMemoryMaterializationStore is a thread-safe in-memory implementation of MaterializationStore.
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
type InMemoryMaterializationStore struct {
	// storage: unit -> materialization -> data
	storage map[string]map[string]*materializationData
	mu      sync.RWMutex
	logger  *slog.Logger
}

type materializationData struct {
	included      bool              // whether unit is in materialized segment
	ruleToVariant map[string]string // rule -> variant mappings for sticky assignments
}

// NewInMemoryMaterializationStore creates a new in-memory materialization store.
func NewInMemoryMaterializationStore(logger *slog.Logger) *InMemoryMaterializationStore {
	if logger == nil {
		logger = slog.Default()
	}
	return &InMemoryMaterializationStore{
		storage: make(map[string]map[string]*materializationData),
		logger:  logger,
	}
}

// Read performs a batch read of materialization data.
func (s *InMemoryMaterializationStore) Read(ctx context.Context, ops []ReadOp) ([]ReadResult, error) {
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
func (s *InMemoryMaterializationStore) Write(ctx context.Context, ops []WriteOp) error {
	s.mu.Lock()
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
func (s *InMemoryMaterializationStore) Close() error {
	s.mu.Lock()
	defer s.mu.Unlock()

	s.storage = make(map[string]map[string]*materializationData)
	s.logger.Debug("In-memory storage cleared")
	return nil
}

// Dump returns all stored materialization data as ReadResults for testing assertions.
func (s *InMemoryMaterializationStore) Dump() []ReadResult {
	s.mu.RLock()
	defer s.mu.RUnlock()

	var results []ReadResult
	for unit, matMap := range s.storage {
		for mat, data := range matMap {
			// Add variant results for each rule
			for rule, variant := range data.ruleToVariant {
				variantCopy := variant
				results = append(results, &ReadResultVariant{
					materialization: mat,
					unit:            unit,
					rule:            rule,
					variant:         &variantCopy,
				})
			}
		}
	}
	return results
}
