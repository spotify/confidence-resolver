package confidence

import (
	"reflect"
	"testing"

	"google.golang.org/protobuf/types/known/structpb"
)

func TestDecodeIntChecks(t *testing.T) {
	for name, tc := range map[string]struct {
		n   float64
		typ any
		ok  bool
	}{
		"fits int8":      {127, int8(0), true},
		"overflows int8": {300, int8(0), false},
		"negative uint":  {-1, uint(0), false},
		"fraction":       {1.5, int(0), false},
		"fits uint16":    {65535, uint16(0), true},
	} {
		_, err := decodeTo(structpb.NewNumberValue(tc.n), reflect.TypeOf(tc.typ), nil)
		if (err == nil) != tc.ok {
			t.Errorf("%s: err=%v", name, err)
		}
	}
}

func TestEncodeDefaultSchema(t *testing.T) {
	type inner struct{ HTTPServer string }
	type flag struct {
		UserID int `json:"uid"`
		Ratio  float32
		Inner  inner
		hidden bool
	}
	v, schema, err := encodeDefault(flag{UserID: 7, Inner: inner{HTTPServer: "x"}})
	if err != nil {
		t.Fatal(err)
	}
	fields := schema.GetStructSchema().GetSchema()
	if fields["uid"].GetIntSchema() == nil || fields["ratio"].GetDoubleSchema() == nil ||
		fields["inner"].GetStructSchema().GetSchema()["http_server"].GetStringSchema() == nil || len(fields) != 3 {
		t.Errorf("schema %v", schema)
	}
	if v.GetStructValue().GetFields()["uid"].GetNumberValue() != 7 {
		t.Errorf("value %v", v)
	}
	if _, schema, _ := encodeDefault(map[string]int{"a": 1}); schema != nil {
		t.Errorf("map got a schema: %v", schema)
	}
	if _, _, err := encodeDefault([]int{1}); err == nil {
		t.Error("slice default accepted")
	}
}

// Values under a map have no schema for the module to check, so decode checks every kind itself.
func TestDecodeChecksKindsUnderMaps(t *testing.T) {
	type label struct{ Label int }
	type named struct{ Label string }
	nested, _ := structpb.NewValue(map[string]any{"nested": map[string]any{"label": "inner"}})
	for name, tc := range map[string]struct {
		def any
		ok  bool
	}{
		"struct in map, string into int":    {map[string]label{}, false},
		"pointer in map, string into int":   {map[string]*label{}, false},
		"map in map, string into bool":      {map[string]map[string]bool{}, false},
		"struct in map, matching":           {map[string]named{}, true},
		"map in map, matching":              {map[string]map[string]string{}, true},
		"map in map, any":                   {map[string]map[string]any{}, true},
		"struct in map, struct into string": {map[string]string{}, false},
	} {
		_, err := decode(nested, tc.def)
		if (err == nil) != tc.ok {
			t.Errorf("%s: err=%v", name, err)
		}
	}
}
