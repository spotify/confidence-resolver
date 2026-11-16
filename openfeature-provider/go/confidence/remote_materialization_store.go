package confidence

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"time"

	"google.golang.org/protobuf/encoding/protowire"
)

// The hosted materialization store, as the JS provider uses it: protobuf over HTTP on the resolver
// gateway. The five small messages are encoded by hand to avoid generating the old internal protos.

const (
	defaultRemoteStoreURL = "https://resolver.confidence.dev"
	remoteReadTimeout     = 2 * time.Second
	remoteWriteTimeout    = 5 * time.Second
)

type remoteMaterializationStore struct {
	baseURL, secret string
	client          *http.Client
}

func newRemoteMaterializationStore(baseURL, secret string, client *http.Client) *remoteMaterializationStore {
	if baseURL == "" {
		baseURL = defaultRemoteStoreURL
	}
	if client == nil {
		client = http.DefaultClient
	}
	return &remoteMaterializationStore{baseURL: baseURL, secret: secret, client: client}
}

// Message fields (confidence.flags.resolver.v1, internal_api.proto).
const (
	readRequestOps     = 3 // ReadOperationsRequest.ops
	readOpVariant      = 1 // ReadOp.variant_read_op
	readOpInclusion    = 2 // ReadOp.inclusion_read_op
	readResultResults  = 1 // ReadOperationsResult.results
	resultVariant      = 1 // ReadResult.variant_result (VariantData)
	resultInclusion    = 2 // ReadResult.inclusion_result (InclusionData)
	writeRequestOps    = 1 // WriteOperationsRequest.store_variant_op (VariantData)
	fUnit, fMat, fRule = 1, 2, 3
	fVariant           = 4 // VariantData.variant
	fIncluded          = 3 // InclusionData.is_included
)

func (s *remoteMaterializationStore) Read(ctx context.Context, ops []ReadOp) ([]ReadResult, error) {
	if len(ops) == 0 {
		return nil, nil
	}
	var req []byte
	for _, op := range ops {
		var inner []byte
		inner = appendString(inner, fUnit, op.Unit())
		inner = appendString(inner, fMat, op.Materialization())
		kind := protowire.Number(readOpInclusion)
		if v, ok := op.(*ReadOpVariant); ok {
			inner = appendString(inner, fRule, v.Rule())
			kind = readOpVariant
		}
		req = protowire.AppendTag(req, readRequestOps, protowire.BytesType)
		req = protowire.AppendBytes(req, protowire.AppendBytes(protowire.AppendTag(nil, kind, protowire.BytesType), inner))
	}
	body, err := s.post(ctx, "readMaterializedOperations", req, remoteReadTimeout)
	if err != nil {
		return nil, err
	}
	var results []ReadResult
	err = fields(body, func(num protowire.Number, v []byte, _ uint64) error {
		if num != readResultResults {
			return nil
		}
		return fields(v, func(kind protowire.Number, v []byte, _ uint64) error {
			var unit, mat, rule, variant string
			var included bool
			err := fields(v, func(num protowire.Number, v []byte, varint uint64) error {
				switch num {
				case fUnit:
					unit = string(v)
				case fMat:
					mat = string(v)
				case fRule:
					if kind == resultVariant {
						rule = string(v)
					} else {
						included = varint != 0
					}
				case fVariant:
					variant = string(v)
				}
				return nil
			})
			switch kind {
			case resultVariant:
				results = append(results, &ReadResultVariant{materialization: mat, unit: unit, rule: rule, variant: &variant})
			case resultInclusion:
				results = append(results, &ReadResultInclusion{materialization: mat, unit: unit, included: included})
			}
			return err
		})
	})
	return results, err
}

func (s *remoteMaterializationStore) Write(ctx context.Context, ops []WriteOp) error {
	var req []byte
	for _, op := range ops {
		v, ok := op.(*WriteOpVariant)
		if !ok {
			return fmt.Errorf("unsupported write op %T", op)
		}
		var inner []byte
		inner = appendString(inner, fUnit, v.Unit())
		inner = appendString(inner, fMat, v.Materialization())
		inner = appendString(inner, fRule, v.Rule())
		inner = appendString(inner, fVariant, v.Variant())
		req = protowire.AppendTag(req, writeRequestOps, protowire.BytesType)
		req = protowire.AppendBytes(req, inner)
	}
	_, err := s.post(ctx, "writeMaterializedOperations", req, remoteWriteTimeout)
	return err
}

func (s *remoteMaterializationStore) post(ctx context.Context, method string, body []byte, timeout time.Duration) ([]byte, error) {
	if _, ok := ctx.Deadline(); !ok {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, timeout)
		defer cancel()
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, s.baseURL+"/v1/materialization:"+method, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/x-protobuf")
	req.Header.Set("Authorization", "ClientSecret "+s.secret)
	resp, err := s.client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("materialization %s: %w", method, err)
	}
	defer resp.Body.Close()
	out, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("materialization %s: %s", method, resp.Status)
	}
	return out, nil
}

func appendString(b []byte, num protowire.Number, s string) []byte {
	if s == "" {
		return b
	}
	return protowire.AppendString(protowire.AppendTag(b, num, protowire.BytesType), s)
}

var errMalformed = errors.New("malformed materialization response")

// fields calls f for each length-delimited (v) or varint field in b; other wire types are skipped.
func fields(b []byte, f func(num protowire.Number, v []byte, varint uint64) error) error {
	for len(b) > 0 {
		num, typ, n := protowire.ConsumeTag(b)
		if n < 0 {
			return errMalformed
		}
		b = b[n:]
		var v []byte
		var varint uint64
		switch typ {
		case protowire.BytesType:
			v, n = protowire.ConsumeBytes(b)
		case protowire.VarintType:
			varint, n = protowire.ConsumeVarint(b)
		default:
			n = protowire.ConsumeFieldValue(num, typ, b)
		}
		if n < 0 {
			return errMalformed
		}
		b = b[n:]
		if err := f(num, v, varint); err != nil {
			return err
		}
	}
	return nil
}
