package host

import (
	"testing"

	"google.golang.org/protobuf/proto"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/resolverpb"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/wasmpb"
)

func TestDecodeEnvelope(t *testing.T) {
	marshal := func(m *wasmpb.ResolveProcessResponse) []byte {
		b, err := proto.Marshal(m)
		if err != nil {
			t.Fatal(err)
		}
		return b
	}
	rec := &wasmpb.Materialization_Record{Unit: "u", Materialization: "m", Rule: "r", Variant: "v"}

	resolved, err := decodeEnvelope(marshal(&wasmpb.ResolveProcessResponse{Result: &wasmpb.ResolveProcessResponse_Resolved_{
		Resolved: &wasmpb.ResolveProcessResponse_Resolved{
			Response:               &resolverpb.ResolveFlagsResponse{ResolveToken: []byte("ignored")},
			MaterializationToWrite: []*wasmpb.Materialization_Record{rec, rec},
			ClientIndex:            3,
		}}}))
	if err != nil || !resolved.resolved || len(resolved.writes) != 2 || !proto.Equal(resolved.writes[0], rec) ||
		resolved.clientIndex != 3 || len(resolved.response) == 0 {
		t.Errorf("resolved: %+v, %v", resolved, err)
	}

	suspended, err := decodeEnvelope(marshal(&wasmpb.ResolveProcessResponse{Result: &wasmpb.ResolveProcessResponse_Suspended_{
		Suspended: &wasmpb.ResolveProcessResponse_Suspended{ProcessId: 7, MaterializationToRead: []*wasmpb.Materialization_Record{rec}},
	}}))
	if err != nil || suspended.suspended == nil || suspended.suspended.processID != 7 || len(suspended.suspended.reads) != 1 {
		t.Errorf("suspended: %+v, %v", suspended, err)
	}

	rejected, err := decodeEnvelope(marshal(&wasmpb.ResolveProcessResponse{Result: &wasmpb.ResolveProcessResponse_Error_{
		Error: &wasmpb.ResolveProcessResponse_Error{Code: wasmpb.ResolveProcessResponse_Error_INVALID_ARGUMENT, Message: "bad"},
	}}))
	if err != nil || rejected.err == nil || rejected.err.Code != wasmpb.ResolveProcessResponse_Error_INVALID_ARGUMENT || rejected.err.Message != "bad" {
		t.Errorf("error: %+v, %v", rejected, err)
	}

	for name, b := range map[string][]byte{"empty": nil, "truncated": {0x0a, 0x05, 0x01}, "bad tag": {0xff}} {
		if _, err := decodeEnvelope(b); err == nil {
			t.Errorf("%s: expected an error", name)
		}
	}
}
