package host

import (
	"errors"

	"google.golang.org/protobuf/encoding/protowire"
	"google.golang.org/protobuf/proto"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/wasmpb"
)

// envelope is the part of a ResolveProcessResponse the host acts on. The resolved
// ResolveFlagsResponse itself is never decoded: evaluate reads it in place from WASM memory.
// This mirrors the Java host's ProcessResponseDecoder.
type envelope struct {
	resolved    bool
	response    []byte                           // resolved: the ResolveFlagsResponse bytes, a view into WASM memory
	writes      []*wasmpb.Materialization_Record // resolved
	clientIndex uint32                           // resolved
	suspended   *suspension
	err         *ResolveError
}

type suspension struct {
	processID uint32
	reads     []*wasmpb.Materialization_Record
}

var errBadEnvelope = errors.New("malformed process response")

// ResolveProcessResponse field numbers.
const (
	fieldResolved, fieldSuspended, fieldError = 1, 2, 3
	// Resolved
	fieldResolvedResponse, fieldResolvedWrites, fieldClientIndex = 1, 2, 3
	// Suspended
	fieldProcessID, fieldReads = 1, 2
)

func decodeEnvelope(b []byte) (envelope, error) {
	var env envelope
	err := eachField(b, func(num protowire.Number, typ protowire.Type, v []byte, varint uint64) error {
		switch {
		case num == fieldResolved && typ == protowire.BytesType:
			env.resolved = true
			return eachField(v, func(num protowire.Number, typ protowire.Type, v []byte, varint uint64) error {
				switch {
				case num == fieldResolvedResponse && typ == protowire.BytesType:
					env.response = v
				case num == fieldResolvedWrites && typ == protowire.BytesType:
					return appendRecord(&env.writes, v)
				case num == fieldClientIndex && typ == protowire.VarintType:
					env.clientIndex = uint32(varint)
				}
				return nil
			})
		case num == fieldSuspended && typ == protowire.BytesType:
			s := &suspension{}
			env.suspended = s
			return eachField(v, func(num protowire.Number, typ protowire.Type, v []byte, varint uint64) error {
				switch {
				case num == fieldProcessID && typ == protowire.VarintType:
					s.processID = uint32(varint)
				case num == fieldReads && typ == protowire.BytesType:
					return appendRecord(&s.reads, v)
				}
				return nil
			})
		case num == fieldError && typ == protowire.BytesType:
			var e wasmpb.ResolveProcessResponse_Error
			if err := proto.Unmarshal(v, &e); err != nil {
				return err
			}
			env.err = &ResolveError{Code: e.GetCode(), Message: e.GetMessage()}
		}
		return nil
	})
	if err == nil && !env.resolved && env.suspended == nil && env.err == nil {
		err = errors.New("empty process response")
	}
	return env, err
}

func appendRecord(dst *[]*wasmpb.Materialization_Record, b []byte) error {
	r := &wasmpb.Materialization_Record{}
	if err := proto.Unmarshal(b, r); err != nil {
		return err
	}
	*dst = append(*dst, r)
	return nil
}

// eachField calls f for every field in b. Length-delimited values are passed as v and varints as
// varint. Other wire types are skipped.
func eachField(b []byte, f func(num protowire.Number, typ protowire.Type, v []byte, varint uint64) error) error {
	for len(b) > 0 {
		num, typ, n := protowire.ConsumeTag(b)
		if n < 0 {
			return errBadEnvelope
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
			return errBadEnvelope
		}
		b = b[n:]
		if err := f(num, typ, v, varint); err != nil {
			return err
		}
	}
	return nil
}
