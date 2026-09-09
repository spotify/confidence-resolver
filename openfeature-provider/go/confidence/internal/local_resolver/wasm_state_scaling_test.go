package local_resolver

import (
	"bytes"
	"compress/gzip"
	"context"
	"fmt"
	"math/rand"
	"os"
	"strconv"
	"strings"
	"testing"
	"time"

	adminv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/admin"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	typesv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/types"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/structpb"
	"google.golang.org/protobuf/types/known/timestamppb"
)

const (
	stateScalingEnv         = "CONFIDENCE_WASM_STATE_SCALING"
	stateScalingSizesEnv    = "CONFIDENCE_WASM_STATE_SCALING_MIB"
	stateScalingModesEnv    = "CONFIDENCE_WASM_STATE_SCALING_MODES"
	stateScalingResolvesEnv = "CONFIDENCE_WASM_STATE_SCALING_RESOLVES"
	stateGrowthEnv          = "CONFIDENCE_WASM_STATE_GROWTH"
	stateGrowthStepEnv      = "CONFIDENCE_WASM_STATE_GROWTH_STEP_MIB"
	stateGrowthMaxEnv       = "CONFIDENCE_WASM_STATE_GROWTH_MAX_MIB"
	stateGrowthModeEnv      = "CONFIDENCE_WASM_STATE_GROWTH_MODE"
	stateGrowthLimitEnv     = "CONFIDENCE_WASM_STATE_GROWTH_MEMORY_LIMIT_MIB"
	productionBitsetBytes   = 1_000_000 / 8
	stateScalingProbes      = 16
)

type bitsetMode string

const (
	bitsetRandom bitsetMode = "random"
	bitsetSparse bitsetMode = "sparse"
	bitsetZero   bitsetMode = "zero"
)

type scalingState struct {
	encoded         []byte
	rawBitsetBytes  int64
	gzipBitsetBytes int64
	bitsetCount     int
	request         *wasm.ResolveProcessRequest
}

type scalingResult struct {
	phase            string
	mode             bitsetMode
	targetMiB        int
	bitsets          int
	rawBytes         int64
	gzipBytes        int64
	stateBytes       int
	memoryBefore     uint64
	memoryAfterState uint64
	memoryAfterRun   uint64
	loadDuration     time.Duration
	resolveDuration  time.Duration
	err              any
}

// TestWasmStateScaling is an opt-in diagnostic harness for measuring the WASM
// linear-memory high-water mark as packed bitsets grow. It is deliberately not
// part of the normal test suite because useful runs allocate hundreds of MiB and
// boundary runs are expected to trap or exhaust the host process.
//
// Run a bounded sample with:
//
//	CONFIDENCE_WASM_STATE_SCALING=1 \
//	CONFIDENCE_WASM_STATE_SCALING_MIB=1,4,16,64 \
//	go test ./confidence/internal/local_resolver -run TestWasmStateScaling -v -count=1
//
// Modes are random (approximately 50% set and incompressible), sparse
// (approximately 1% set), and zero. Override them with
// CONFIDENCE_WASM_STATE_SCALING_MODES. Each state uses 125,000-byte bitsets,
// matching the resolver's 1,000,000-bucket universe.
func TestWasmStateScaling(t *testing.T) {
	if os.Getenv(stateScalingEnv) == "" {
		t.Skipf("set %s=1 to run the WASM state scaling harness", stateScalingEnv)
	}

	sizes := parsePositiveInts(t, stateScalingSizesEnv, "1,4,16,64")
	modes := parseBitsetModes(t, os.Getenv(stateScalingModesEnv))
	resolveCount := parsePositiveInt(t, stateScalingResolvesEnv, "1000")

	factory := NewWasmResolverFactory(NoOpLogSink, false)
	t.Cleanup(func() { _ = factory.Close(context.Background()) })

	t.Log("phase,mode,target_mib,bitsets,raw_bytes,gzip_bytes,state_bytes,memory_before,memory_after_state,memory_after_resolves,load_ms,resolve_ns_per_op,status")
	for _, mode := range modes {
		results := make([]scalingResult, 0, len(sizes))
		for _, sizeMiB := range sizes {
			state := buildScalingState(t, sizeMiB, mode)
			result := measureFreshScalingState(factory, state, mode, sizeMiB, resolveCount)
			logScalingResult(t, result, resolveCount)
			results = append(results, result)
		}
		logScalingSlope(t, "fresh", mode, results)

		// Loading increasing states into one instance captures allocator reuse and
		// the old-state + new-state transient peak. Linear memory never shrinks,
		// so this series must be interpreted separately from the fresh instances.
		resolverInstance := factory.New()
		wasmResolver := resolverInstance.(*WasmResolver)
		for _, sizeMiB := range sizes {
			state := buildScalingState(t, sizeMiB, mode)
			result := measureScalingState(wasmResolver, state, "replace", mode, sizeMiB, resolveCount)
			logScalingResult(t, result, resolveCount)
			if result.err != nil {
				break
			}
		}
		safeCloseScalingResolver(resolverInstance)
	}
}

// TestWasmStateContinuousGrowth repeatedly replaces one instance's complete
// state while appending more deterministic bitsets on every update. Larger
// generated states preserve every bitset from the preceding state and add a
// suffix, matching full-state polling more closely than independent samples.
//
// A test-only memory limit makes it possible to observe the guest allocation
// failure without allowing one diagnostic run to consume the host's entire
// address space. Set the limit to 0 to exercise the runtime's normal maximum.
func TestWasmStateContinuousGrowth(t *testing.T) {
	if os.Getenv(stateGrowthEnv) == "" {
		t.Skipf("set %s=1 to run the continuous WASM state growth harness", stateGrowthEnv)
	}

	stepMiB := parsePositiveInt(t, stateGrowthStepEnv, "8")
	maxMiB := parsePositiveInt(t, stateGrowthMaxEnv, "512")
	resolveCount := parsePositiveInt(t, stateScalingResolvesEnv, "100")
	limitMiB := parseNonNegativeInt(t, stateGrowthLimitEnv, "0")
	mode := parseSingleBitsetMode(t, os.Getenv(stateGrowthModeEnv), bitsetRandom)

	factory := newScalingWasmResolverFactory(t, limitMiB)
	t.Cleanup(func() { _ = factory.Close(context.Background()) })
	resolverInstance := factory.New()
	wasmResolver := resolverInstance.(*WasmResolver)
	defer safeCloseScalingResolver(resolverInstance)

	t.Log("phase,mode,target_mib,bitsets,raw_bytes,gzip_bytes,state_bytes,memory_before,memory_after_state,memory_after_resolves,load_ms,resolve_ns_per_op,status")
	lastSuccessMiB := 0
	for sizeMiB := stepMiB; sizeMiB <= maxMiB; sizeMiB += stepMiB {
		state := buildScalingState(t, sizeMiB, mode)
		result := measureScalingState(wasmResolver, state, "continuous", mode, sizeMiB, resolveCount)
		logScalingResult(t, result, resolveCount)
		if result.err != nil {
			t.Logf("continuous growth stopped: last_success_target_mib=%d first_failure_target_mib=%d memory_limit_mib=%d", lastSuccessMiB, sizeMiB, limitMiB)
			return
		}
		lastSuccessMiB = sizeMiB
	}
	t.Logf("continuous growth reached configured maximum: last_success_target_mib=%d memory_limit_mib=%d", lastSuccessMiB, limitMiB)
}

func newScalingWasmResolverFactory(t *testing.T, limitMiB int) *WasmResolverFactory {
	t.Helper()
	if limitMiB == 0 {
		return NewWasmResolverFactory(NoOpLogSink, false).(*WasmResolverFactory)
	}

	pages := uint64(limitMiB) * 16
	if pages > 65_536 {
		t.Fatalf("%s cannot exceed 4096 MiB, got %d", stateGrowthLimitEnv, limitMiB)
	}
	ctx := context.Background()
	runtimeConfig := wazero.NewRuntimeConfig().WithMemoryLimitPages(uint32(pages))
	runtime := wazero.NewRuntimeWithConfig(ctx, runtimeConfig)
	_, err := runtime.NewHostModuleBuilder("wasm_msg").
		NewFunctionBuilder().
		WithFunc(func(ctx context.Context, mod api.Module, ptr uint32) uint32 {
			consumeRequest(mod, ptr)
			return transferResponseSuccess(mod, mustMarshal(timestamppb.Now()))
		}).
		Export("wasm_msg_host_current_time").
		Instantiate(ctx)
	if err != nil {
		_ = runtime.Close(ctx)
		t.Fatalf("instantiate scaling host module: %v", err)
	}
	module, err := runtime.CompileModule(ctx, wasmBytes)
	if err != nil {
		_ = runtime.Close(ctx)
		t.Fatalf("compile resolver module: %v", err)
	}
	return &WasmResolverFactory{runtime: runtime, module: module, logSink: NoOpLogSink}
}

func measureFreshScalingState(
	factory LocalResolverFactory,
	state scalingState,
	mode bitsetMode,
	sizeMiB int,
	resolveCount int,
) scalingResult {
	resolverInstance := factory.New()
	wasmResolver := resolverInstance.(*WasmResolver)
	result := measureScalingState(wasmResolver, state, "fresh", mode, sizeMiB, resolveCount)
	safeCloseScalingResolver(resolverInstance)
	return result
}

func safeCloseScalingResolver(resolver LocalResolver) {
	// A boundary run can leave the instance trapping on every call, including
	// the best-effort flush performed by Close. The diagnostic result is more
	// useful than a second panic while cleaning up that instance.
	defer func() { _ = recover() }()
	_ = resolver.Close(context.Background())
}

func measureScalingState(
	wasmResolver *WasmResolver,
	state scalingState,
	phase string,
	mode bitsetMode,
	sizeMiB int,
	resolveCount int,
) (result scalingResult) {
	result = scalingResult{
		phase:        phase,
		mode:         mode,
		targetMiB:    sizeMiB,
		bitsets:      state.bitsetCount,
		rawBytes:     state.rawBitsetBytes,
		gzipBytes:    state.gzipBitsetBytes,
		stateBytes:   len(state.encoded),
		memoryBefore: wasmLinearMemoryBytes(wasmResolver),
	}
	loadStart := time.Now()

	// Calls into WasmResolver panic on a WASM trap. Preserve the last readable
	// memory size and print a row rather than losing all earlier measurements.
	defer func() {
		if recovered := recover(); recovered != nil {
			result.err = recovered
			result.loadDuration = time.Since(loadStart)
			result.memoryAfterRun = wasmLinearMemoryBytes(wasmResolver)
		}
	}()

	if err := wasmResolver.SetResolverState(&wasm.SetResolverStateRequest{
		State:     state.encoded,
		AccountId: "scaling-account",
	}); err != nil {
		result.err = err
		return result
	}
	result.loadDuration = time.Since(loadStart)
	result.memoryAfterState = wasmLinearMemoryBytes(wasmResolver)

	resolveStart := time.Now()
	for i := 0; i < resolveCount; i++ {
		if _, err := wasmResolver.ResolveProcess(state.request); err != nil {
			result.err = err
			break
		}
		if i%100 == 99 {
			_ = wasmResolver.FlushAllLogs()
		}
	}
	result.resolveDuration = time.Since(resolveStart)
	result.memoryAfterRun = wasmLinearMemoryBytes(wasmResolver)
	return result
}

func wasmLinearMemoryBytes(resolver *WasmResolver) uint64 {
	// wazero Memory.Size returns uint32 and explicitly wraps to zero at the
	// memory32 maximum of 65,536 pages. Grow(0) is its documented workaround.
	pages, ok := resolver.instance.Memory().Grow(0)
	if !ok {
		return uint64(resolver.instance.Memory().Size())
	}
	return uint64(pages) * 65_536
}

func buildScalingState(t *testing.T, targetMiB int, mode bitsetMode) scalingState {
	t.Helper()

	targetBytes := int64(targetMiB) << 20
	bitsetCount := int((targetBytes + productionBitsetBytes - 1) / productionBitsetBytes)
	segments := make([]*adminv1.Segment, 0, bitsetCount)
	bitsets := make([]*adminv1.ResolverState_PackedBitset, 0, bitsetCount)
	// The seed intentionally does not depend on targetMiB: a larger state has
	// the exact bitset prefix of a smaller state and only appends new segments.
	rng := rand.New(rand.NewSource(0x5eed + modeSeed(mode)))
	raw := make([]byte, productionBitsetBytes)
	var gzipBytes int64

	for i := 0; i < bitsetCount; i++ {
		fillBitset(raw, mode, rng)
		compressed := gzipBytesForTest(t, raw)
		name := fmt.Sprintf("segments/scaling-%06d", i)
		segments = append(segments, &adminv1.Segment{Name: name})
		bitsets = append(bitsets, &adminv1.ResolverState_PackedBitset{
			Segment: name,
			Bitset: &adminv1.ResolverState_PackedBitset_GzippedBitset{
				GzippedBitset: compressed,
			},
		})
		gzipBytes += int64(len(compressed))
	}

	probeCount := min(stateScalingProbes, bitsetCount)
	flags := make([]*adminv1.Flag, 0, probeCount)
	flagNames := make([]string, 0, probeCount)
	for i := 0; i < probeCount; i++ {
		segmentIndex := i * bitsetCount / probeCount
		flag := scalingFlag(i, segments[segmentIndex].Name)
		flags = append(flags, flag)
		flagNames = append(flagNames, flag.Name)
	}

	state := &adminv1.ResolverState{
		Flags:             flags,
		SegmentsNoBitsets: segments,
		Bitsets:           bitsets,
		Clients:           []*adminv1.Client{{Name: "clients/scaling"}},
		ClientCredentials: []*adminv1.ClientCredential{{
			Name: "clients/scaling/credentials/test",
			Credential: &adminv1.ClientCredential_ClientSecret_{
				ClientSecret: &adminv1.ClientCredential_ClientSecret{Secret: "scaling-secret"},
			},
		}},
	}
	encoded, err := proto.Marshal(state)
	if err != nil {
		t.Fatalf("marshal scaling state: %v", err)
	}

	request := &wasm.ResolveProcessRequest{
		Resolve: &wasm.ResolveProcessRequest_WithoutMaterializations{
			WithoutMaterializations: &resolver.ResolveFlagsRequest{
				Flags:        flagNames,
				ClientSecret: "scaling-secret",
				EvaluationContext: &structpb.Struct{Fields: map[string]*structpb.Value{
					"user_id": structpb.NewStringValue("scaling-user"),
				}},
			},
		},
	}

	return scalingState{
		encoded:         encoded,
		rawBitsetBytes:  int64(bitsetCount * productionBitsetBytes),
		gzipBitsetBytes: gzipBytes,
		bitsetCount:     bitsetCount,
		request:         request,
	}
}

func scalingFlag(index int, segment string) *adminv1.Flag {
	name := fmt.Sprintf("flags/scaling-%02d", index)
	variant := name + "/variants/on"
	return &adminv1.Flag{
		Name:    name,
		State:   adminv1.Flag_ACTIVE,
		Clients: []string{"clients/scaling"},
		Variants: []*adminv1.Flag_Variant{{
			Name: variant,
			Value: &structpb.Struct{Fields: map[string]*structpb.Value{
				"enabled": structpb.NewBoolValue(true),
			}},
		}},
		Rules: []*adminv1.Flag_Rule{{
			Name:                 name + "/rules/scaling",
			Segment:              segment,
			TargetingKeySelector: "user_id",
			Enabled:              true,
			AssignmentSpec: &adminv1.Flag_Rule_AssignmentSpec{
				BucketCount: 1,
				Assignments: []*adminv1.Flag_Rule_Assignment{{
					AssignmentId: "on",
					Assignment: &adminv1.Flag_Rule_Assignment_Variant{
						Variant: &adminv1.Flag_Rule_Assignment_VariantAssignment{Variant: variant},
					},
					BucketRanges: []*adminv1.Flag_Rule_BucketRange{{Lower: 0, Upper: 1}},
				}},
			},
		}},
		Schema: &typesv1.FlagSchema_StructFlagSchema{},
	}
}

func fillBitset(dst []byte, mode bitsetMode, rng *rand.Rand) {
	clear(dst)
	switch mode {
	case bitsetRandom:
		_, _ = rng.Read(dst)
	case bitsetSparse:
		// One set bit per 100 bits on average, without a floating-point draw
		// for every individual bit.
		for bit := 0; bit < len(dst)*8; bit += 100 {
			position := bit + rng.Intn(100)
			if position < len(dst)*8 {
				dst[position/8] |= 1 << (position % 8)
			}
		}
	case bitsetZero:
	default:
		panic("unsupported bitset mode: " + mode)
	}
}

func gzipBytesForTest(t *testing.T, raw []byte) []byte {
	t.Helper()
	var dst bytes.Buffer
	writer, err := gzip.NewWriterLevel(&dst, gzip.BestSpeed)
	if err != nil {
		t.Fatalf("create gzip writer: %v", err)
	}
	if _, err := writer.Write(raw); err != nil {
		t.Fatalf("gzip bitset: %v", err)
	}
	if err := writer.Close(); err != nil {
		t.Fatalf("close gzip writer: %v", err)
	}
	return dst.Bytes()
}

func logScalingResult(t *testing.T, result scalingResult, resolveCount int) {
	t.Helper()
	status := "ok"
	if result.err != nil {
		status = fmt.Sprintf("error:%v", result.err)
	}
	var resolveNs int64
	if resolveCount > 0 {
		resolveNs = result.resolveDuration.Nanoseconds() / int64(resolveCount)
	}
	t.Logf("%s,%s,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%s",
		result.phase,
		result.mode,
		result.targetMiB,
		result.bitsets,
		result.rawBytes,
		result.gzipBytes,
		result.stateBytes,
		result.memoryBefore,
		result.memoryAfterState,
		result.memoryAfterRun,
		result.loadDuration.Milliseconds(),
		resolveNs,
		status,
	)
}

func logScalingSlope(t *testing.T, phase string, mode bitsetMode, results []scalingResult) {
	t.Helper()
	var first, last *scalingResult
	for i := range results {
		if results[i].err != nil {
			continue
		}
		if first == nil {
			first = &results[i]
		}
		last = &results[i]
	}
	if first == nil || last == nil || first == last || last.rawBytes == first.rawBytes {
		return
	}
	slope := float64(last.memoryAfterState-first.memoryAfterState) / float64(last.rawBytes-first.rawBytes)
	t.Logf("summary phase=%s mode=%s linear_memory_bytes_per_raw_bitset_byte=%.3f", phase, mode, slope)
}

func parsePositiveInts(t *testing.T, name, fallback string) []int {
	t.Helper()
	value := os.Getenv(name)
	if value == "" {
		value = fallback
	}
	parts := strings.Split(value, ",")
	result := make([]int, 0, len(parts))
	for _, part := range parts {
		parsed, err := strconv.Atoi(strings.TrimSpace(part))
		if err != nil || parsed <= 0 {
			t.Fatalf("%s must be a comma-separated list of positive integers, got %q", name, value)
		}
		result = append(result, parsed)
	}
	return result
}

func parsePositiveInt(t *testing.T, name, fallback string) int {
	t.Helper()
	value := os.Getenv(name)
	if value == "" {
		value = fallback
	}
	parsed, err := strconv.Atoi(strings.TrimSpace(value))
	if err != nil || parsed <= 0 {
		t.Fatalf("%s must be a positive integer, got %q", name, value)
	}
	return parsed
}

func parseNonNegativeInt(t *testing.T, name, fallback string) int {
	t.Helper()
	value := os.Getenv(name)
	if value == "" {
		value = fallback
	}
	parsed, err := strconv.Atoi(strings.TrimSpace(value))
	if err != nil || parsed < 0 {
		t.Fatalf("%s must be a non-negative integer, got %q", name, value)
	}
	return parsed
}

func parseSingleBitsetMode(t *testing.T, value string, fallback bitsetMode) bitsetMode {
	t.Helper()
	if value == "" {
		return fallback
	}
	modes := parseBitsetModes(t, value)
	if len(modes) != 1 {
		t.Fatalf("%s must contain exactly one mode, got %q", stateGrowthModeEnv, value)
	}
	return modes[0]
}

func parseBitsetModes(t *testing.T, value string) []bitsetMode {
	t.Helper()
	if value == "" {
		value = "random,sparse,zero"
	}
	parts := strings.Split(value, ",")
	result := make([]bitsetMode, 0, len(parts))
	for _, part := range parts {
		mode := bitsetMode(strings.TrimSpace(part))
		switch mode {
		case bitsetRandom, bitsetSparse, bitsetZero:
			result = append(result, mode)
		default:
			t.Fatalf("%s contains unsupported mode %q", stateScalingModesEnv, mode)
		}
	}
	return result
}

func modeSeed(mode bitsetMode) int64 {
	switch mode {
	case bitsetRandom:
		return 1
	case bitsetSparse:
		return 2
	case bitsetZero:
		return 3
	default:
		return 0
	}
}
