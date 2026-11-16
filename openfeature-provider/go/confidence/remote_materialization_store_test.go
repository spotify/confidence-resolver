package confidence

import (
	"bytes"
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestRemoteStoreWire(t *testing.T) {
	// ReadOperationsRequest{ops: [ReadOp{inclusion_read_op{unit: "user-a", materialization: "m"}}]}
	wantRead := []byte{0x1a, 0x0d, 0x12, 0x0b, 0x0a, 0x06, 'u', 's', 'e', 'r', '-', 'a', 0x12, 0x01, 'm'}
	// ReadOperationsResult{results: [ReadResult{inclusion_result{unit, materialization, is_included: true}}]}
	readResp := []byte{0x0a, 0x0f, 0x12, 0x0d, 0x0a, 0x06, 'u', 's', 'e', 'r', '-', 'a', 0x12, 0x01, 'm', 0x18, 0x01}
	var gotPath, gotAuth, gotType string
	var gotBody []byte
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotPath, gotAuth, gotType = r.URL.Path, r.Header.Get("Authorization"), r.Header.Get("Content-Type")
		gotBody, _ = io.ReadAll(r.Body)
		_, _ = w.Write(readResp)
	}))
	defer srv.Close()
	s := newRemoteMaterializationStore(srv.URL, "sec", srv.Client())

	res, err := s.Read(context.Background(), []ReadOp{newReadOpInclusion("m", "user-a")})
	if err != nil {
		t.Fatal(err)
	}
	if gotPath != "/v1/materialization:readMaterializedOperations" || gotAuth != "ClientSecret sec" || gotType != "application/x-protobuf" {
		t.Errorf("request %s %q %q", gotPath, gotAuth, gotType)
	}
	if !bytes.Equal(gotBody, wantRead) {
		t.Errorf("read body % x, want % x", gotBody, wantRead)
	}
	if inc, ok := res[0].(*ReadResultInclusion); len(res) != 1 || !ok || !inc.Included() || inc.Unit() != "user-a" {
		t.Errorf("results %+v", res)
	}

	if err := s.Write(context.Background(), []WriteOp{newWriteOpVariant("m", "u", "r", "v")}); err != nil {
		t.Fatal(err)
	}
	wantWrite := []byte{0x0a, 0x0c, 0x0a, 0x01, 'u', 0x12, 0x01, 'm', 0x1a, 0x01, 'r', 0x22, 0x01, 'v'}
	if gotPath != "/v1/materialization:writeMaterializedOperations" || !bytes.Equal(gotBody, wantWrite) {
		t.Errorf("write %s % x", gotPath, gotBody)
	}
}
