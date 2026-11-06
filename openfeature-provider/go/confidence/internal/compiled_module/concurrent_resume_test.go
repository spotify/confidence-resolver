package compiled_module

import (
	"sync"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/flagswasm"
	"google.golang.org/protobuf/types/known/structpb"
)

// TestHostConcurrentSuspendResumeAndDiscard drives many goroutines through
// full suspend flows at once. With unpinned suspensions, the resume runs on
// whatever pool instance is free — a different instance than the one that
// started the process — and response blocks are freed cross-instance. The
// module's process state is module-wide in the shared memory, so this is
// the contract-sanctioned concurrency model (the reference host does the
// same).
func TestHostConcurrentSuspendResumeAndDiscard(t *testing.T) {
	host, _ := newE2EHost(t)

	request := e2eRequest(t, true)
	stickyMaterialization := &flagswasm.Materialization{
		Records: []*flagswasm.Materialization_Record{stickyRecord()},
	}

	const goroutines = 8
	const iterations = 20
	var wg sync.WaitGroup
	for g := 0; g < goroutines; g++ {
		wg.Add(1)
		go func(g int) {
			defer wg.Done()
			for i := 0; i < iterations; i++ {
				result, suspended, err := host.StartResolve(request)
				if err != nil {
					t.Errorf("StartResolve: %v", err)
					return
				}
				if result != nil {
					t.Error("sticky resolve resolved without suspending")
					result.Envelope.Close()
					return
				}

				if (g+i)%2 == 0 {
					final, err := host.ResumeResolve(suspended, stickyMaterialization)
					if err != nil {
						t.Errorf("ResumeResolve: %v", err)
						return
					}
					flags := final.Response.GetResolvedFlags()
					if len(flags) != 1 || flags[0].GetVariant() != "flags/web-sdk-e2e-flag/variants/sticky" {
						t.Errorf("resumed variant = %v, want sticky", flags)
						final.Envelope.Close()
						return
					}
					details, err := host.Evaluate(final.Envelope, &flagswasm.EvaluateRequest{
						FlagKey:      "web-sdk-e2e-flag.str",
						DefaultValue: structpb.NewStringValue("fallback"),
					})
					final.Envelope.Close()
					if err != nil {
						t.Errorf("Evaluate: %v", err)
						return
					}
					if details.GetErrorCode() != 0 {
						t.Errorf("evaluate error code = %s", details.GetErrorCode())
						return
					}
				} else {
					host.Discard(suspended)
				}
			}
		}(g)
	}
	wg.Wait()
}
