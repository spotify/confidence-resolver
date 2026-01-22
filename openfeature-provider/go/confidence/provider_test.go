package confidence

import (
	"context"
	"testing"

	"github.com/open-feature/go-sdk/openfeature"
	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
	"google.golang.org/protobuf/types/known/structpb"
)

func TestNewLocalResolverProvider(t *testing.T) {
	provider := NewLocalResolverProvider(nil, nil, nil, "test-secret", nil)

	if provider == nil {
		t.Fatal("Expected provider to be created, got nil")
	}
	if provider.clientSecret != "test-secret" {
		t.Errorf("Expected client secret to be 'test-secret', got %s", provider.clientSecret)
	}
}

func TestLocalResolverProvider_Metadata(t *testing.T) {
	provider := NewLocalResolverProvider(nil, nil, nil, "secret", nil)
	metadata := provider.Metadata()

	if metadata.Name != "confidence-sdk-go-local" {
		t.Errorf("Expected provider name to be 'confidence-sdk-go-local', got %s", metadata.Name)
	}
}

func TestLocalResolverProvider_Hooks(t *testing.T) {
	provider := NewLocalResolverProvider(nil, nil, nil, "secret", nil)
	hooks := provider.Hooks()

	if hooks == nil {
		t.Error("Expected hooks to not be nil")
	}
	if len(hooks) != 0 {
		t.Errorf("Expected 0 hooks, got %d", len(hooks))
	}
}

func TestParseFlagPath(t *testing.T) {
	testCases := []struct {
		name         string
		input        string
		expectedFlag string
		expectedPath string
	}{
		{
			name:         "Simple flag name",
			input:        "my-flag",
			expectedFlag: "my-flag",
			expectedPath: "",
		},
		{
			name:         "Flag with path",
			input:        "my-flag.nested.value",
			expectedFlag: "my-flag",
			expectedPath: "nested.value",
		},
		{
			name:         "Flag with single level path",
			input:        "flag.value",
			expectedFlag: "flag",
			expectedPath: "value",
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			flag, path := parseFlagPath(tc.input)
			if flag != tc.expectedFlag {
				t.Errorf("Expected flag '%s', got '%s'", tc.expectedFlag, flag)
			}
			if path != tc.expectedPath {
				t.Errorf("Expected path '%s', got '%s'", tc.expectedPath, path)
			}
		})
	}
}

func TestProcessTargetingKey(t *testing.T) {
	testCases := []struct {
		name     string
		input    openfeature.FlattenedContext
		expected map[string]interface{}
	}{
		{
			name: "Converts targetingKey to targeting_key",
			input: openfeature.FlattenedContext{
				"targetingKey": "user-123",
				"other":        "value",
			},
			expected: map[string]interface{}{
				"targeting_key": "user-123",
				"other":         "value",
			},
		},
		{
			name: "No targetingKey",
			input: openfeature.FlattenedContext{
				"other": "value",
			},
			expected: map[string]interface{}{
				"other": "value",
			},
		},
		{
			name:     "Empty context",
			input:    openfeature.FlattenedContext{},
			expected: map[string]interface{}{},
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			result := processTargetingKey(tc.input)

			if len(result) != len(tc.expected) {
				t.Errorf("Expected %d keys, got %d", len(tc.expected), len(result))
			}

			for key, expectedValue := range tc.expected {
				if result[key] != expectedValue {
					t.Errorf("Expected key '%s' to have value '%v', got '%v'", key, expectedValue, result[key])
				}
			}

			// Ensure targetingKey is not present if it was in input
			if _, hasTargetingKey := tc.input["targetingKey"]; hasTargetingKey {
				if _, stillHas := result["targetingKey"]; stillHas {
					t.Error("Expected targetingKey to be removed")
				}
			}
		})
	}
}

func TestGoValueToProto(t *testing.T) {
	testCases := []struct {
		name        string
		input       interface{}
		expectError bool
	}{
		{
			name:        "Nil value",
			input:       nil,
			expectError: false,
		},
		{
			name:        "Bool value",
			input:       true,
			expectError: false,
		},
		{
			name:        "Int value",
			input:       42,
			expectError: false,
		},
		{
			name:        "Int64 value",
			input:       int64(42),
			expectError: false,
		},
		{
			name:        "Float64 value",
			input:       3.14,
			expectError: false,
		},
		{
			name:        "String value",
			input:       "hello",
			expectError: false,
		},
		{
			name:        "Array value",
			input:       []interface{}{1, "two", true},
			expectError: false,
		},
		{
			name: "Map value",
			input: map[string]interface{}{
				"key": "value",
			},
			expectError: false,
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			result, err := goValueToProto(tc.input)
			if tc.expectError {
				if err == nil {
					t.Error("Expected error, got nil")
				}
			} else {
				if err != nil {
					t.Errorf("Expected no error, got %v", err)
				}
				if result == nil {
					t.Error("Expected result to not be nil")
				}
			}
		})
	}
}

func TestProtoValueToGo(t *testing.T) {
	testCases := []struct {
		name     string
		input    *structpb.Value
		expected interface{}
	}{
		{
			name:     "Nil value",
			input:    nil,
			expected: nil,
		},
		{
			name:     "Null value",
			input:    structpb.NewNullValue(),
			expected: nil,
		},
		{
			name:     "Bool value",
			input:    structpb.NewBoolValue(true),
			expected: true,
		},
		{
			name:     "Number value",
			input:    structpb.NewNumberValue(42.5),
			expected: 42.5,
		},
		{
			name:     "String value",
			input:    structpb.NewStringValue("hello"),
			expected: "hello",
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			result := protoValueToGo(tc.input)
			if result != tc.expected {
				t.Errorf("Expected %v, got %v", tc.expected, result)
			}
		})
	}
}

func TestProtoStructToGo(t *testing.T) {
	// Test nil struct
	result := protoStructToGo(nil)
	if result != nil {
		t.Error("Expected nil result for nil struct")
	}

	// Test struct with values
	pbStruct := &structpb.Struct{
		Fields: map[string]*structpb.Value{
			"name":   structpb.NewStringValue("test"),
			"count":  structpb.NewNumberValue(42),
			"active": structpb.NewBoolValue(true),
		},
	}

	result = protoStructToGo(pbStruct)
	if result == nil {
		t.Fatal("Expected non-nil result")
	}

	resultMap, ok := result.(map[string]interface{})
	if !ok {
		t.Fatal("Expected result to be map[string]interface{}")
	}

	if resultMap["name"] != "test" {
		t.Errorf("Expected name to be 'test', got %v", resultMap["name"])
	}
	if resultMap["count"] != 42.0 {
		t.Errorf("Expected count to be 42.0, got %v", resultMap["count"])
	}
	if resultMap["active"] != true {
		t.Errorf("Expected active to be true, got %v", resultMap["active"])
	}
}

func TestGetValueForPath(t *testing.T) {
	testData := map[string]interface{}{
		"level1": map[string]interface{}{
			"level2": map[string]interface{}{
				"level3": "deep-value",
			},
			"simple": "simple-value",
		},
		"top":     "top-value",
		"nullval": nil,
	}

	testCases := []struct {
		name          string
		path          string
		expected      interface{}
		expectedFound bool
		checkIsMap    bool
	}{
		{
			name:          "Empty path",
			path:          "",
			expected:      nil, // Will check map separately
			expectedFound: true,
			checkIsMap:    true,
		},
		{
			name:          "Top level value",
			path:          "top",
			expected:      "top-value",
			expectedFound: true,
		},
		{
			name:          "Nested value",
			path:          "level1.simple",
			expected:      "simple-value",
			expectedFound: true,
		},
		{
			name:          "Deep nested value",
			path:          "level1.level2.level3",
			expected:      "deep-value",
			expectedFound: true,
		},
		{
			name:          "Non-existent path",
			path:          "does.not.exist",
			expected:      nil,
			expectedFound: false,
		},
		{
			name:          "Null value at path",
			path:          "nullval",
			expected:      nil,
			expectedFound: true,
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			result, found := getValueForPath(tc.path, testData)
			if found != tc.expectedFound {
				t.Errorf("Expected found=%v, got found=%v", tc.expectedFound, found)
			}
			if tc.checkIsMap {
				// For empty path, should return the original map
				if _, ok := result.(map[string]interface{}); !ok {
					t.Errorf("Expected result to be a map, got %T", result)
				}
			} else {
				if result != tc.expected {
					t.Errorf("Expected %v, got %v", tc.expected, result)
				}
			}
		})
	}
}

func TestGetValueForPath_NonMapValue(t *testing.T) {
	// Test with non-map value in path
	testData := map[string]interface{}{
		"value": "string-value",
	}

	result, found := getValueForPath("value.nested", testData)
	if found {
		t.Errorf("Expected found=false for path through non-map value, got found=true")
	}
	if result != nil {
		t.Errorf("Expected nil for path through non-map value, got %v", result)
	}
}

func TestFlattenedContextToProto(t *testing.T) {
	ctx := openfeature.FlattenedContext{
		"string": "value",
		"number": 42,
		"bool":   true,
	}

	result, err := flattenedContextToProto(ctx)
	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}
	if result == nil {
		t.Fatal("Expected non-nil result")
	}
	if len(result.Fields) != 3 {
		t.Errorf("Expected 3 fields, got %d", len(result.Fields))
	}
}

func TestFlattenedContextToProto_InvalidValue(t *testing.T) {
	ctx := openfeature.FlattenedContext{
		"invalid": make(chan int), // Channels cannot be converted
	}

	_, err := flattenedContextToProto(ctx)
	if err == nil {
		t.Error("Expected error for invalid value type")
	}
}

func TestLocalResolverProvider_Shutdown(t *testing.T) {
	provider := NewLocalResolverProvider(nil, nil, nil, "secret", nil)
	provider.Shutdown()

	// Verify the method can be called without panicking even with nil components
	// The provider manages its own lifecycle now
}

func TestLocalResolverProvider_ShutdownWithCancelFunc(t *testing.T) {
	provider := NewLocalResolverProvider(nil, nil, nil, "secret", nil)

	// Simulate Init having been called by setting cancelFunc
	cancelCalled := false
	_, cancel := context.WithCancel(context.Background())
	provider.mu.Lock()
	provider.cancelFunc = func() {
		cancelCalled = true
		cancel()
	}
	provider.mu.Unlock()

	// Shutdown should call cancel
	provider.Shutdown()

	// Verify that cancel was called
	if !cancelCalled {
		t.Error("Expected cancel function to be called")
	}
}

// Mock implementations for Init() testing

type mockResolverAPIForInit struct {
	updateStateFunc   func(state []byte, accountID string) error
	closeFunc         func(ctx context.Context)
	resolveWithSticky func(request *wasm.ResolveWithStickyRequest) (*wasm.ResolveWithStickyResponse, error)
}

func mockResolverSupplier(_ context.Context, _ lr.LogSink) lr.LocalResolver {
	return &mockResolverAPIForInit{}
}

func (m *mockResolverAPIForInit) SetResolverState(request *wasm.SetResolverStateRequest) error {
	if m.updateStateFunc != nil {
		return m.updateStateFunc(request.State, request.AccountId)
	}
	return nil
}

func (m *mockResolverAPIForInit) Close(ctx context.Context) error {
	if m.closeFunc != nil {
		m.closeFunc(ctx)
	}
	return nil
}

func (m *mockResolverAPIForInit) ResolveWithSticky(request *wasm.ResolveWithStickyRequest) (*wasm.ResolveWithStickyResponse, error) {
	if m.resolveWithSticky != nil {
		return m.resolveWithSticky(request)
	}
	return nil, nil
}

func (m *mockResolverAPIForInit) FlushAllLogs() error {
	return nil
}

func (m *mockResolverAPIForInit) FlushAssignLogs() error {
	return nil
}

// TestLocalResolverProvider_Init_NilStateProvider verifies Init fails when stateProvider is nil
func TestLocalResolverProvider_Init_NilStateProvider(t *testing.T) {
	provider := NewLocalResolverProvider(
		mockResolverSupplier,
		nil, // nil state provider
		&tu.MockFlagLogger{},
		"secret",
		nil,
	)

	err := provider.Init(openfeature.EvaluationContext{})
	if err == nil {
		t.Fatal("Expected error when stateProvider is nil")
	}
	if err.Error() != "state provider is nil, cannot initialize" {
		t.Errorf("Expected specific error message, got: %v", err)
	}
}

// TestLocalResolverProvider_Init_NilResolverAPI verifies Init fails when resolverAPI is nil
func TestLocalResolverProvider_Init_NilResolverAPI(t *testing.T) {
	provider := NewLocalResolverProvider(
		nil, // nil resolver API
		&tu.StateProviderMock{},
		&tu.MockFlagLogger{},
		"secret",
		nil,
	)

	err := provider.Init(openfeature.EvaluationContext{})
	if err == nil {
		t.Fatal("Expected error when resolverAPI is nil")
	}
	if err.Error() != "resolverSupplier is nil, cannot initialize" {
		t.Errorf("Expected specific error message, got: %v", err)
	}
}

// TestLocalResolverProvider_Init_NilFlagLogger verifies Init fails when FlagLogger is nil
func TestLocalResolverProvider_Init_NilFlagLogger(t *testing.T) {
	provider := NewLocalResolverProvider(
		mockResolverSupplier,
		&tu.StateProviderMock{},
		nil, // nil flag logger
		"secret",
		nil,
	)

	err := provider.Init(openfeature.EvaluationContext{})
	if err == nil {
		t.Fatal("Expected error when flagLogger is nil")
	}
	if err.Error() != "Flag logger is nil,  cannot initialize" {
		t.Errorf("Expected specific error message, got: %v", err)
	}
}

// TestLocalResolverProvider_Init_StateProviderError verifies Init fails when stateProvider.Provide returns error
func TestLocalResolverProvider_Init_StateProviderError(t *testing.T) {
	mockStateProvider := &tu.StateProviderMock{
		State:     []byte("cached-state"),
		AccountID: "cached-account",
		Err:       context.DeadlineExceeded,
	}

	mockFlagLogger := &tu.MockFlagLogger{}

	provider := NewLocalResolverProvider(
		mockResolverSupplier,
		mockStateProvider,
		mockFlagLogger,
		"secret",
		nil,
	)

	err := provider.Init(openfeature.EvaluationContext{})
	if err == nil {
		t.Fatal("Expected error when stateProvider.Provide fails")
	}
	// Should wrap the original error
	if err.Error() != "failed to fetch initial state: context deadline exceeded" {
		t.Errorf("Expected wrapped error message, got: %v", err)
	}
}

// TestLocalResolverProvider_Init_EmptyAccountID verifies Init fails when accountID is empty
func TestLocalResolverProvider_Init_EmptyAccountID(t *testing.T) {
	mockStateProvider := &tu.StateProviderMock{
		State:     []byte("test-state"),
		AccountID: "",
		Err:       nil,
	}

	mockFlagLogger := &tu.MockFlagLogger{}

	mockResolverAPI := &mockResolverAPIForInit{}

	provider := NewLocalResolverProvider(
		func(ctx context.Context, ls lr.LogSink) lr.LocalResolver { return mockResolverAPI },
		mockStateProvider,
		mockFlagLogger,
		"secret",
		nil,
	)

	err := provider.Init(openfeature.EvaluationContext{})
	if err == nil {
		t.Fatal("Expected error when accountID is empty")
	}
	if err.Error() != "AccountID is empty in the initial state" {
		t.Errorf("Expected specific error message, got: %v", err)
	}
}

// TestLocalResolverProvider_Init_UpdateStateError verifies Init fails when UpdateStateAndFlushLogs fails
func TestLocalResolverProvider_Init_UpdateStateError(t *testing.T) {
	mockStateProvider := &tu.StateProviderMock{
		State:     []byte("test-state"),
		AccountID: "test-account",
		Err:       nil,
	}

	mockFlagLogger := &tu.MockFlagLogger{}

	mockResolver := &mockResolverAPIForInit{
		updateStateFunc: func(state []byte, accountID string) error {
			return context.DeadlineExceeded
		},
	}

	mockResolverSupplier := func(_ context.Context, _ lr.LogSink) lr.LocalResolver {
		return mockResolver
	}

	provider := NewLocalResolverProvider(
		mockResolverSupplier,
		mockStateProvider,
		mockFlagLogger,
		"secret",
		nil,
	)

	err := provider.Init(openfeature.EvaluationContext{})
	if err == nil {
		t.Fatal("Expected error when UpdateStateAndFlushLogs fails")
	}
	if err.Error() != "failed to initialize resolver: context deadline exceeded" {
		t.Errorf("Expected wrapped error message, got: %v", err)
	}
}

// TestLocalResolverProvider_Init_Success verifies successful Init
func TestLocalResolverProvider_Init_Success(t *testing.T) {
	updateStateCalled := false
	var receivedState []byte
	var receivedAccountID string

	mockStateProvider := &tu.StateProviderMock{
		State:     []byte("test-state-data"),
		AccountID: "test-account-123",
		Err:       nil,
	}

	mockResolver := &mockResolverAPIForInit{
		updateStateFunc: func(state []byte, accountID string) error {
			updateStateCalled = true
			receivedState = state
			receivedAccountID = accountID
			return nil
		},
	}

	mockFlagLogger := &tu.MockFlagLogger{}

	mockResolverSupplier := func(_ context.Context, _ lr.LogSink) lr.LocalResolver {
		return mockResolver
	}

	provider := NewLocalResolverProvider(
		mockResolverSupplier,
		mockStateProvider,
		mockFlagLogger,
		"secret",
		nil,
	)

	err := provider.Init(openfeature.EvaluationContext{})
	if err != nil {
		t.Fatalf("Expected no error, got: %v", err)
	}

	if !updateStateCalled {
		t.Error("Expected UpdateStateAndFlushLogs to be called")
	}

	if string(receivedState) != "test-state-data" {
		t.Errorf("Expected state to be 'test-state-data', got: %s", string(receivedState))
	}

	if receivedAccountID != "test-account-123" {
		t.Errorf("Expected accountID to be 'test-account-123', got: %s", receivedAccountID)
	}

	// Verify background tasks were started (cancelFunc should be set)
	provider.mu.Lock()
	hasCancelFunc := provider.cancelFunc != nil
	provider.mu.Unlock()

	if !hasCancelFunc {
		t.Error("Expected cancelFunc to be set after Init")
	}

	// Clean up
	provider.Shutdown()
}
