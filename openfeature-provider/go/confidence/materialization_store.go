package confidence

import (
	"context"
)

// MaterializationStore provides persistent storage for materialization data.
//
// Materializations support two key use cases:
//  1. Sticky Assignments: Maintain consistent variant assignments across evaluations
//     even when targeting attributes change. This enables pausing intake (stopping new
//     users from entering an experiment) while keeping existing users in their assigned variants.
//  2. Custom Targeting via Materialized Segments: Precomputed sets of identifiers from
//     datasets that should be targeted. Instead of evaluating complex targeting rules at
//     runtime, materializations allow efficient lookup of whether a unit (user, session, etc.)
//     is included in a target segment.
//
// Custom Implementations: Optionally implement this interface to store materialization
// data in your own infrastructure (Redis, database, etc.) to eliminate network calls
// and improve latency during flag resolution.
//
// Thread Safety: Implementations must be safe for concurrent use by multiple goroutines.
//
// Example Implementation: See InMemoryMaterializationStore for a reference implementation.
//
// Key Concepts:
//   - Materialization: An identifier for a materialization context (experiment, feature flag, or materialized segment)
//   - Unit: The entity identifier (user ID, session ID, etc.)
//   - Rule: The targeting rule identifier within a flag
//   - Variant: The assigned variant name for the unit+rule combination
type MaterializationStore interface {
	// Read performs a batch read of materialization data.
	// The resolver calls this method to fetch stored materialization data, including sticky
	// assignments and materialized segment memberships.
	Read(ctx context.Context, ops []ReadOp) ([]ReadResult, error)

	// Write performs a batch write of materialization data.
	// The resolver calls this method to persist materialization data after successful flag
	// resolution. This includes storing sticky variant assignments and materialized segment
	// memberships. Implementations should be idempotent.
	// Default implementations may return UnsupportedOperationError.
	Write(ctx context.Context, ops []WriteOp) error
}

// ReadOp represents a read operation to query materialization data.
type ReadOp interface {
	// Materialization returns the materialization identifier
	Materialization() string
	// Unit returns the unit identifier (user ID, session ID, etc.)
	Unit() string
	// isReadOp is a marker method for type safety
	isReadOp()
}

// ReadOpInclusion is a query operation to check if a unit is included in a materialized segment.
// Used for custom targeting to efficiently determine if a unit is part of a precomputed target set.
type ReadOpInclusion struct {
	materialization string
	unit            string
}

// newReadOpInclusion creates a new inclusion read operation.
func newReadOpInclusion(materialization, unit string) *ReadOpInclusion {
	return &ReadOpInclusion{
		materialization: materialization,
		unit:            unit,
	}
}

func (r *ReadOpInclusion) Materialization() string { return r.materialization }
func (r *ReadOpInclusion) Unit() string            { return r.unit }
func (r *ReadOpInclusion) isReadOp()               {}

// ToResult converts a boolean result into a properly typed ReadResult.
func (r *ReadOpInclusion) ToResult(included bool) ReadResult {
	return &ReadResultInclusion{
		materialization: r.materialization,
		unit:            r.unit,
		included:        included,
	}
}

// ReadOpVariant is a query operation to retrieve the variant assignment for a unit and rule.
// Used for sticky assignments to fetch the previously assigned variant.
type ReadOpVariant struct {
	materialization string
	unit            string
	rule            string
}

// newReadOpVariant creates a new variant read operation.
func newReadOpVariant(materialization, unit, rule string) *ReadOpVariant {
	return &ReadOpVariant{
		materialization: materialization,
		unit:            unit,
		rule:            rule,
	}
}

func (r *ReadOpVariant) Materialization() string { return r.materialization }
func (r *ReadOpVariant) Unit() string            { return r.unit }
func (r *ReadOpVariant) Rule() string            { return r.rule }
func (r *ReadOpVariant) isReadOp()               {}

// ToResult converts an optional variant into a properly typed ReadResult.
func (r *ReadOpVariant) ToResult(variant *string) ReadResult {
	return &ReadResultVariant{
		materialization: r.materialization,
		unit:            r.unit,
		rule:            r.rule,
		variant:         variant,
	}
}

// ReadResult represents the result of a read operation.
type ReadResult interface {
	// Materialization returns the materialization identifier
	Materialization() string
	// Unit returns the unit identifier
	Unit() string
	// isReadResult is a marker method for type safety
	isReadResult()
}

// ReadResultInclusion indicates whether a unit is included in a materialized segment.
// Used for custom targeting via materialized segments - efficient lookup to determine if a
// unit (user, session, etc.) is part of a precomputed target set.
type ReadResultInclusion struct {
	materialization string
	unit            string
	included        bool
}

func (r *ReadResultInclusion) Materialization() string { return r.materialization }
func (r *ReadResultInclusion) Unit() string            { return r.unit }
func (r *ReadResultInclusion) Included() bool          { return r.included }
func (r *ReadResultInclusion) isReadResult()           {}

// ReadResultVariant contains the variant assignment for a unit and rule.
// Used for sticky assignments - returns the previously assigned variant for a unit and
// targeting rule combination.
type ReadResultVariant struct {
	materialization string
	unit            string
	rule            string
	variant         *string // nil if no assignment exists
}

func (r *ReadResultVariant) Materialization() string { return r.materialization }
func (r *ReadResultVariant) Unit() string            { return r.unit }
func (r *ReadResultVariant) Rule() string            { return r.rule }
func (r *ReadResultVariant) Variant() *string        { return r.variant }
func (r *ReadResultVariant) isReadResult()           {}

// WriteOp represents a write operation to store materialization data.
type WriteOp interface {
	// Materialization returns the materialization identifier
	Materialization() string
	// Unit returns the unit identifier
	Unit() string
	// isWriteOp is a marker method for type safety
	isWriteOp()
}

// WriteOpVariant is a variant assignment write operation.
// Used to store sticky variant assignments, recording which variant a unit (user, session, etc.)
// should receive for a specific targeting rule.
type WriteOpVariant struct {
	materialization string
	unit            string
	rule            string
	variant         string
}

// newWriteOpVariant creates a new variant write operation.
func newWriteOpVariant(materialization, unit, rule, variant string) *WriteOpVariant {
	return &WriteOpVariant{
		materialization: materialization,
		unit:            unit,
		rule:            rule,
		variant:         variant,
	}
}

func (w *WriteOpVariant) Materialization() string { return w.materialization }
func (w *WriteOpVariant) Unit() string            { return w.unit }
func (w *WriteOpVariant) Rule() string            { return w.rule }
func (w *WriteOpVariant) Variant() string         { return w.variant }
func (w *WriteOpVariant) isWriteOp()              {}

// MaterializationNotSupportedError is returned when a MaterializationStore doesn't support
// the requested operation. This triggers the provider to fall back to remote gRPC resolution
// via the Confidence service, which manages materializations server-side.
type MaterializationNotSupportedError struct {
	Message string
}

func (e *MaterializationNotSupportedError) Error() string {
	if e.Message != "" {
		return e.Message
	}
	return "materialization operation not supported"
}
