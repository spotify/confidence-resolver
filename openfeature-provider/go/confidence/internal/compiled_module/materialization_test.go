package compiled_module

import (
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/flagswasm"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"google.golang.org/protobuf/types/known/structpb"
)

// newE2EHost loads the e2e fixture (two credentials, sticky
// materialization rule on flags/web-sdk-e2e-flag).
func newE2EHost(t *testing.T) (*Host, *logCollector) {
	t.Helper()
	wasm := loadFixture(t, "e2e_client_module.wasm")
	mod, err := LoadModule(wasm, e2eSecretA)
	if err != nil {
		t.Fatalf("LoadModule: %v", err)
	}
	collector := &logCollector{}
	host, err := NewHost(mod, HostConfig{LogSink: collector.sink})
	if err != nil {
		t.Fatalf("NewHost: %v", err)
	}
	t.Cleanup(host.Close)
	return host, collector
}

func e2eRequest(t *testing.T, sticky bool) *resolver.ResolveFlagsRequest {
	t.Helper()
	evalCtx, err := structpb.NewStruct(map[string]interface{}{
		"targeting_key": "test-a",
		"sticky":        sticky,
	})
	if err != nil {
		t.Fatalf("NewStruct: %v", err)
	}
	return &resolver.ResolveFlagsRequest{
		Flags:             []string{"flags/web-sdk-e2e-flag"},
		EvaluationContext: evalCtx,
		ClientSecret:      e2eSecretA,
		Apply:             true,
	}
}

func stickyRecord() *flagswasm.Materialization_Record {
	return &flagswasm.Materialization_Record{
		Unit:            "test-a",
		Materialization: "materializedSegments/2b5776c3c0517893a48c7081ee159d31",
		Rule:            "flags/web-sdk-e2e-flag/rules/ilp2u0ovfx87kyalomtn",
		Variant:         "flags/web-sdk-e2e-flag/variants/sticky",
	}
}

func TestHostSuspendAndResume(t *testing.T) {
	host, _ := newE2EHost(t)

	result, suspended, err := host.StartResolve(e2eRequest(t, true))
	if err != nil {
		t.Fatalf("StartResolve: %v", err)
	}
	if result != nil {
		result.Envelope.Close()
	}
	if suspended == nil {
		t.Fatal("expected sticky resolve to suspend for materializations")
	}
	if suspended.processID == 0 {
		t.Fatal("suspended process has no handle")
	}
	if len(suspended.ToRead()) == 0 {
		t.Fatal("suspended process lists no materializations to read")
	}

	final, err := host.ResumeResolve(suspended, &flagswasm.Materialization{
		Records: []*flagswasm.Materialization_Record{stickyRecord()},
	})
	if err != nil {
		t.Fatalf("ResumeResolve: %v", err)
	}
	defer final.Envelope.Close()

	flags := final.Response.GetResolvedFlags()
	if len(flags) != 1 {
		t.Fatalf("resolved %d flags, want 1", len(flags))
	}
	if got := flags[0].GetVariant(); got != "flags/web-sdk-e2e-flag/variants/sticky" {
		t.Errorf("variant = %q, want sticky", got)
	}
	if flags[0].GetReason() != resolver.ResolveReason_RESOLVE_REASON_MATCH {
		t.Errorf("reason = %s", flags[0].GetReason())
	}
}

func TestHostResumeWithoutMaterialization(t *testing.T) {
	host, _ := newE2EHost(t)

	_, suspended, err := host.StartResolve(e2eRequest(t, true))
	if err != nil {
		t.Fatalf("StartResolve: %v", err)
	}
	if suspended == nil {
		t.Fatal("expected suspension")
	}

	// Absent records count as absent: the flag resolves with a
	// missing-materializations reason rather than an error.
	final, err := host.ResumeResolve(suspended, &flagswasm.Materialization{})
	if err != nil {
		t.Fatalf("ResumeResolve: %v", err)
	}
	defer final.Envelope.Close()
	flags := final.Response.GetResolvedFlags()
	if len(flags) != 1 {
		t.Fatalf("resolved %d flags, want 1", len(flags))
	}
	if flags[0].GetReason() != resolver.ResolveReason_RESOLVE_REASON_MATERIALIZATION_NOT_SUPPORTED {
		t.Errorf("reason = %s, want MATERIALIZATION_NOT_SUPPORTED", flags[0].GetReason())
	}
}

func TestHostDiscardSuspended(t *testing.T) {
	host, _ := newE2EHost(t)

	_, suspended, err := host.StartResolve(e2eRequest(t, true))
	if err != nil {
		t.Fatalf("StartResolve: %v", err)
	}
	if suspended == nil {
		t.Fatal("expected suspension")
	}
	host.Discard(suspended)

	// A discarded process must not poison later resolves.
	result, _, err := host.StartResolve(e2eRequest(t, false))
	if err != nil {
		t.Fatalf("resolve after discard: %v", err)
	}
	result.Envelope.Close()
}

// TestHostResumeTwiceFails resumes a consumed handle: the module returns an
// empty response, which surfaces as an error rather than a bad resolve.
func TestHostResumeTwiceFails(t *testing.T) {
	host, _ := newE2EHost(t)

	_, suspended, err := host.StartResolve(e2eRequest(t, true))
	if err != nil {
		t.Fatalf("StartResolve: %v", err)
	}
	final, err := host.ResumeResolve(suspended, &flagswasm.Materialization{
		Records: []*flagswasm.Materialization_Record{stickyRecord()},
	})
	if err != nil {
		t.Fatalf("ResumeResolve: %v", err)
	}
	final.Envelope.Close()

	if _, err := host.ResumeResolve(suspended, &flagswasm.Materialization{}); err == nil {
		t.Fatal("expected second resume of a consumed handle to fail")
	}
}
