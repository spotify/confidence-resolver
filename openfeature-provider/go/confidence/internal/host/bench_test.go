package host

import (
	"context"
	"fmt"
	"runtime"
	"sync/atomic"
	"testing"

	"google.golang.org/protobuf/types/known/structpb"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/wasmpb"
)

func BenchmarkEvaluate(b *testing.B) {
	for _, tc := range []struct {
		name string
		flag string
		key  string
		recs []*wasmpb.Materialization_Record
	}{
		{"plain", "flags/plain", "plain.enabled", nil},
		{"sticky-suspend", "flags/sticky", "sticky.value", []*wasmpb.Materialization_Record{stickyRecord}},
	} {
		b.Run(tc.name, func(b *testing.B) {
			r, err := NewResolver(context.Background(), fixtureB(b, "basic.wasm"), false,
				Config{Instances: runtime.GOMAXPROCS(0), WriteLogs: func([]byte) {}})
			if err != nil {
				b.Fatal(err)
			}
			defer r.Close(context.Background())
			eval := []*wasmpb.EvaluateRequest{{FlagKey: tc.key, DefaultValue: structpb.NewBoolValue(false)}}
			store := &fakeStore{records: tc.recs}
			var n atomic.Int64
			b.ReportAllocs()
			b.ResetTimer()
			b.RunParallel(func(pb *testing.PB) {
				for pb.Next() {
					req := request(fmt.Sprintf("u%d", n.Add(1)%1000), tc.flag)
					if _, err := r.Evaluate(context.Background(), req, eval, store); err != nil {
						b.Fatal(err)
					}
				}
			})
		})
	}
}
