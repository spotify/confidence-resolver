package confidence

import (
	"reflect"
	"testing"

	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

func TestToSnakeCase(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		// Simple cases
		{"", ""},
		{"foo", "foo"},
		{"Foo", "foo"},
		{"fooBar", "foo_bar"},
		{"FooBar", "foo_bar"},

		// Acronyms
		{"HTTP", "http"},
		{"HTTPServer", "http_server"},
		{"userID", "user_id"},
		{"userIDNumber", "user_id_number"},
		{"getHTTPResponse", "get_http_response"},
		{"XMLHTTPRequest", "xmlhttp_request"},

		// Edge cases
		{"ID", "id"},
		{"URL", "url"},
		{"APIKey", "api_key"},
		{"OAuth2Token", "o_auth2_token"},

		// Already snake_case (passthrough)
		{"already_snake", "already_snake"},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			got := toSnakeCase(tt.input)
			if got != tt.expected {
				t.Errorf("toSnakeCase(%q) = %q, want %q", tt.input, got, tt.expected)
			}
		})
	}
}

func TestUnmarshalProto(t *testing.T) {

	testPositive := func(t *testing.T, name string, protoJson string, defaultValue any, expectedValue any) {
		t.Run(name, func(t *testing.T) {
			got, err := unmarshalProto(tu.MustJSONToProto(protoJson), defaultValue, []string{})
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}

			if !reflect.DeepEqual(got, expectedValue) {
				t.Errorf("got %v, want %v", got, expectedValue)
			}
		})
	}

	testNegative := func(t *testing.T, name string, protoJson string, defaultValue any, expectedErrorMsg string) {
		t.Run(name+" err", func(t *testing.T) {
			got, err := unmarshalProto(tu.MustJSONToProto(protoJson), defaultValue, []string{})
			if err == nil {
				t.Errorf("expected error")
			}
			if !reflect.DeepEqual(got, defaultValue) {
				t.Errorf("expected defaultValue, got %v, defaultValue %v", got, defaultValue)
			}
			if err.Error() != expectedErrorMsg {
				t.Errorf(`got error "%v", want "%v"`, err.Error(), expectedErrorMsg)
			}
		})
	}

	t.Run("simple types", func(t *testing.T) {
		// String
		testPositive(t, "string", `"hej"`, "", "hej")

		// Bool
		testPositive(t, "bool true", `true`, false, true)
		testPositive(t, "bool false", `false`, true, false)

		// Float types
		testPositive(t, "float64", `3.14`, float64(0), float64(3.14))
		testPositive(t, "float32", `2.5`, float32(0), float32(2.5))

		// Signed integers
		testPositive(t, "int", `42`, int(0), int(42))
		testPositive(t, "int8", `127`, int8(0), int8(127))
		testPositive(t, "int16", `1000`, int16(0), int16(1000))
		testPositive(t, "int32", `100000`, int32(0), int32(100000))
		testPositive(t, "int64", `9999999999`, int64(0), int64(9999999999))

		// Unsigned integers
		testPositive(t, "uint", `42`, uint(0), uint(42))
		testPositive(t, "uint8", `255`, uint8(0), uint8(255))
		testPositive(t, "uint16", `65000`, uint16(0), uint16(65000))
		testPositive(t, "uint32", `100000`, uint32(0), uint32(100000))
		testPositive(t, "uint64", `9999999999`, uint64(0), uint64(9999999999))

		// Negative integers
		testPositive(t, "negative int", `-42`, int(0), int(-42))
		testPositive(t, "negative int64", `-9999999999`, int64(0), int64(-9999999999))

		// Null resolved value
		testPositive(t, "null to int", `null`, 3, 3)

		// Negative to unsigned should fail
		testNegative(t, "negative to uint", `-1`, uint(0), "resolved value (-1) is negative, cannot convert to uint")
		// Fraction to int should fail
		testNegative(t, "fraction to int", `3.00001`, int(0), "resolved value (3.00001) is not a whole number, cannot convert to int")
		testNegative(t, "fraction to uint", `3.00001`, uint(0), "resolved value (3.00001) is not a whole number, cannot convert to uint")
		// Type mismatch
		testNegative(t, "boolean to string", `false`, "hej", "resolved value (bool) not assignable to default type (string)")

	})

	t.Run("structs", func(t *testing.T) {
		type inner = struct {
			Field bool
		}
		type someStruct = struct {
			Field string
			Deep  inner
		}

		expected := someStruct{
			Field: "hej",
			Deep: inner{
				Field: true,
			},
		}

		testPositive(t, "struct exact", `{ "field":"hej", "deep":{ "field": true }}`, someStruct{}, expected)
		testPositive(t, "struct exact pointer", `{ "field":"hej", "deep":{ "field": true }}`, &someStruct{}, &expected)

		testPositive(t, "struct resolved null field", `{ "field": null, "deep": { "field": true }}`, someStruct{Field: "hej"}, expected)

		testPositive(t, "struct with missing field", `{ "field":"hej", "deep":{ "field": true }, "extra":true}`, someStruct{}, expected)

		testNegative(t, "struct with extra field", `{ "field":"hej", "deep":{}, "extra":true}`, someStruct{}, "resolved value is missing field deep.field")

		testNegative(t, "struct with wrong field type", `{ "field":"hej", "deep":{ "field": 7 }}`, someStruct{}, "resolved value (number) not assignable to default type (bool), at deep.field")

		// Slice fields are not supported
		type structWithSlice = struct {
			Items []string
		}
		testNegative(t, "struct with slice field", `{ "items": ["a", "b"] }`, structWithSlice{}, "slice types are not supported, at items")

		// Nested struct null uses default
		t.Run("nested struct null", func(t *testing.T) {
			defaultWithNested := someStruct{
				Field: "default",
				Deep:  inner{Field: true},
			}
			// When deep is null, use the default's Deep value
			got, err := unmarshalProto(tu.MustJSONToProto(`{ "field": "hello", "deep": null }`), defaultWithNested, []string{})
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if got.Field != "hello" {
				t.Errorf("expected field 'hello', got %v", got.Field)
			}
			if got.Deep.Field != true {
				t.Errorf("expected deep.field to use default (true), got %v", got.Deep.Field)
			}
		})

		// Pointer fields
		t.Run("pointer fields", func(t *testing.T) {
			type structWithPointers struct {
				Name  *string `json:"name"`
				Count *int    `json:"count"`
			}

			name := "test"
			count := 42
			expected := structWithPointers{Name: &name, Count: &count}

			got, err := unmarshalProto(tu.MustJSONToProto(`{ "name": "test", "count": 42 }`), structWithPointers{}, []string{})
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if got.Name == nil || *got.Name != "test" {
				t.Errorf("expected name 'test', got %v", got.Name)
			}
			if got.Count == nil || *got.Count != 42 {
				t.Errorf("expected count 42, got %v", got.Count)
			}
			if !reflect.DeepEqual(got, expected) {
				t.Errorf("got %+v, want %+v", got, expected)
			}
		})

		// Pointer field with null uses default
		t.Run("pointer field null with default", func(t *testing.T) {
			type structWithPointer struct {
				Value *string `json:"value"`
			}

			defaultVal := "default"
			defaultStruct := structWithPointer{Value: &defaultVal}

			got, err := unmarshalProto(tu.MustJSONToProto(`{ "value": null }`), defaultStruct, []string{})
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if got.Value == nil || *got.Value != "default" {
				t.Errorf("expected default 'default', got %v", got.Value)
			}
		})

		// Empty struct
		t.Run("empty struct", func(t *testing.T) {
			type emptyStruct struct{}
			got, err := unmarshalProto(tu.MustJSONToProto(`{}`), emptyStruct{}, []string{})
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if !reflect.DeepEqual(got, emptyStruct{}) {
				t.Errorf("expected empty struct, got %+v", got)
			}
		})

		t.Run("json tags", func(t *testing.T) {
			type taggedStruct struct {
				CustomName     string `json:"custom_name"`
				WithOptions    int    `json:"with_opts,omitempty"`
				CamelCaseField bool   // should match camel_case_field via snake_case
				NoTag          string // should match no_tag via snake_case
			}

			expected := taggedStruct{
				CustomName:     "value1",
				WithOptions:    42,
				CamelCaseField: true,
				NoTag:          "value2",
			}

			testPositive(t, "json tag override",
				`{ "custom_name": "value1", "with_opts": 42, "camel_case_field": true, "no_tag": "value2" }`,
				taggedStruct{}, expected)

			// Test that wrong json key fails
			testNegative(t, "wrong key name",
				`{ "CustomName": "value1", "with_opts": 42, "camel_case_field": true, "no_tag": "value2" }`,
				taggedStruct{}, "resolved value is missing field custom_name")
		})

	})

	t.Run("maps", func(t *testing.T) {

		expected := map[string]any{
			"field": "hej",
			"deep": map[string]any{
				"field": true,
			},
		}
		expectedExtra := map[string]any{
			"field": "hej",
			"deep": map[string]any{
				"field": true,
			},
			"extra": true,
		}
		defaultValue := map[string]any{
			"field": "",
			"deep": map[string]any{
				"field": false,
			},
		}
		defaultValueExtra := map[string]any{
			"field": "",
			"deep": map[string]any{
				"field": false,
			},
			"extra": true,
		}

		testPositive(t, "map exact", `{ "field":"hej", "deep":{ "field": true }}`, defaultValue, expected)
		testPositive(t, "map nil default", `{ "field":"hej", "deep":{ "field": true }}`, nil, expected)

		// With nil defaultValue, AsInterface() is used - null fields become nil in the resulting map
		expectedWithNull := map[string]any{
			"field":      "value",
			"null_field": nil,
		}
		testPositive(t, "map with null field and nil default", `{ "field":"value", "null_field": null }`, nil, expectedWithNull)

		testPositive(t, "map with missing field", `{ "field":"hej", "deep":{ "field": true }, "extra":true}`, defaultValue, expectedExtra)
		// this works differently from structs where we reject default values with extra fields
		testPositive(t, "map with extra field", `{ "field":"hej", "deep":{ "field": true }}`, defaultValueExtra, expectedExtra)

		testNegative(t, "map with wrong field type", `{ "field":"hej", "deep":{ "field": 3 }}`, defaultValue, "resolved value (number) not assignable to default type (bool), at deep.field")

		// Slice in default value should error
		sliceDefaultValue := map[string]any{
			"items": []string{},
		}
		testNegative(t, "map with slice in default", `{ "items": ["a", "b"] }`, sliceDefaultValue, "slice types are not supported, at items")

		t.Run("conversion", func(t *testing.T) {
			defaultValue := map[string]any{
				"value": 0,
			}
			expectedValue := map[string]any{
				"value": 7,
			}

			testPositive(t, "whole number converts to int", `{ "value": 7.0 }`, defaultValue, expectedValue)
			testNegative(t, "fractional number rejected for int", `{ "value": 7.99999 }`, defaultValue, "resolved value (7.99999) is not a whole number, cannot convert to int, at value")
		})

		// Map with struct values
		t.Run("map with struct values", func(t *testing.T) {
			type item struct {
				Name  string `json:"name"`
				Value int    `json:"value"`
			}

			defaultValue := map[string]item{}
			expected := map[string]item{
				"first":  {Name: "one", Value: 1},
				"second": {Name: "two", Value: 2},
			}

			got, err := unmarshalProto(
				tu.MustJSONToProto(`{ "first": { "name": "one", "value": 1 }, "second": { "name": "two", "value": 2 } }`),
				defaultValue,
				[]string{},
			)
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if !reflect.DeepEqual(got, expected) {
				t.Errorf("got %+v, want %+v", got, expected)
			}
		})

		t.Run("default value not modified", func(t *testing.T) {
			defaultValue := map[string]any{
				"field": "original",
				"deep": map[string]any{
					"nested": "untouched",
				},
			}
			// Deep copy for comparison
			originalCopy := map[string]any{
				"field": "original",
				"deep": map[string]any{
					"nested": "untouched",
				},
			}

			result, err := unmarshalProto(tu.MustJSONToProto(`{ "field": "changed", "deep": { "nested": "modified" } }`), defaultValue, []string{})
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}

			// Verify result has new values
			if result["field"] != "changed" {
				t.Errorf("expected result to have 'changed', got %v", result["field"])
			}

			// Verify defaultValue was not modified
			if !reflect.DeepEqual(defaultValue, originalCopy) {
				t.Errorf("defaultValue was modified: got %v, want %v", defaultValue, originalCopy)
			}

			// Also verify nested map wasn't modified
			deepMap := defaultValue["deep"].(map[string]any)
			if deepMap["nested"] != "untouched" {
				t.Errorf("nested defaultValue was modified: got %v, want 'untouched'", deepMap["nested"])
			}
		})
	})

	t.Run("nil protoValue", func(t *testing.T) {
		// nil protoValue should return defaultValue
		got, err := unmarshalProto[string](nil, "default", []string{})
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if got != "default" {
			t.Errorf("expected 'default', got %v", got)
		}

		// nil protoValue with struct
		type simple struct {
			Field string `json:"field"`
		}
		defaultStruct := simple{Field: "default"}
		gotStruct, err := unmarshalProto[simple](nil, defaultStruct, []string{})
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if gotStruct.Field != "default" {
			t.Errorf("expected field 'default', got %v", gotStruct.Field)
		}
	})

	t.Run("interface field", func(t *testing.T) {
		type structWithInterface struct {
			Data any `json:"data"`
		}

		// Interface field with string default, proto has string
		defaultVal := structWithInterface{Data: ""}
		got, err := unmarshalProto(tu.MustJSONToProto(`{ "data": "hello" }`), defaultVal, []string{})
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if got.Data != "hello" {
			t.Errorf("expected data 'hello', got %v", got.Data)
		}

		// Interface field with int default, proto has int
		defaultValInt := structWithInterface{Data: 0}
		gotInt, err := unmarshalProto(tu.MustJSONToProto(`{ "data": 42 }`), defaultValInt, []string{})
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if gotInt.Data != 42 {
			t.Errorf("expected data 42, got %v", gotInt.Data)
		}

		// Interface field with nil default uses dynamic typing
		defaultValNil := structWithInterface{Data: nil}
		gotNil, err := unmarshalProto(tu.MustJSONToProto(`{ "data": "dynamic" }`), defaultValNil, []string{})
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if gotNil.Data != "dynamic" {
			t.Errorf("expected data 'dynamic', got %v", gotNil.Data)
		}
	})
}
