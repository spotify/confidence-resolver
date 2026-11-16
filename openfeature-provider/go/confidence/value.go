package confidence

import (
	"errors"
	"fmt"
	"math"
	"reflect"
	"strings"

	"google.golang.org/protobuf/types/known/structpb"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/typespb"
)

// The module's evaluate export owns every evaluation decision (type checks, null-to-default merge,
// missing fields, error codes). This file only translates between Go values and the module's
// protobuf values:
//
//   - encodeDefault turns the caller's default into evaluate's default_value, and its Go type into
//     expected_schema (structs with json/snake_case field names, int vs double).
//   - decode turns the returned value into the caller's Go type. The checks it keeps are the ones
//     a FlagSchema can't express: Go integer width and sign, and map element types.

var errUnsupported = errors.New("unsupported default type")

// encodeDefault returns the default as a Value, and a schema for its type, or nil when the type
// can't be described (maps, untyped nil, interface values holding nothing).
func encodeDefault(def any) (*structpb.Value, *typespb.FlagSchema, error) {
	if def == nil {
		return structpb.NewNullValue(), nil, nil
	}
	return encode(reflect.ValueOf(def))
}

func encode(v reflect.Value) (*structpb.Value, *typespb.FlagSchema, error) {
	switch v.Kind() {
	case reflect.Pointer, reflect.Interface:
		if v.IsNil() {
			if v.Kind() == reflect.Pointer {
				_, schema, err := encode(reflect.Zero(v.Type().Elem()))
				return structpb.NewNullValue(), schema, err
			}
			return structpb.NewNullValue(), nil, nil
		}
		return encode(v.Elem())
	case reflect.Bool:
		return structpb.NewBoolValue(v.Bool()), schemaOf(&typespb.FlagSchema_BoolSchema{BoolSchema: &typespb.FlagSchema_BoolFlagSchema{}}), nil
	case reflect.String:
		return structpb.NewStringValue(v.String()), schemaOf(&typespb.FlagSchema_StringSchema{StringSchema: &typespb.FlagSchema_StringFlagSchema{}}), nil
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		return structpb.NewNumberValue(float64(v.Int())), intSchema(), nil
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return structpb.NewNumberValue(float64(v.Uint())), intSchema(), nil
	case reflect.Float32, reflect.Float64:
		return structpb.NewNumberValue(v.Float()), schemaOf(&typespb.FlagSchema_DoubleSchema{DoubleSchema: &typespb.FlagSchema_DoubleFlagSchema{}}), nil
	case reflect.Struct:
		fields := map[string]*structpb.Value{}
		schemas := map[string]*typespb.FlagSchema{}
		for i := range v.NumField() {
			f := v.Type().Field(i)
			if !f.IsExported() {
				continue
			}
			value, schema, err := encode(v.Field(i))
			if err != nil {
				return nil, nil, fmt.Errorf("field %s: %w", f.Name, err)
			}
			if schema == nil {
				return nil, nil, fmt.Errorf("field %s: %w %s", f.Name, errUnsupported, f.Type)
			}
			name := fieldName(f)
			fields[name], schemas[name] = value, schema
		}
		return structpb.NewStructValue(&structpb.Struct{Fields: fields}),
			schemaOf(&typespb.FlagSchema_StructSchema{StructSchema: &typespb.FlagSchema_StructFlagSchema{Schema: schemas}}), nil
	case reflect.Map:
		if v.Type().Key().Kind() != reflect.String {
			return nil, nil, fmt.Errorf("%w %s: map keys must be strings", errUnsupported, v.Type())
		}
		fields := map[string]*structpb.Value{}
		for _, k := range v.MapKeys() {
			value, _, err := encode(v.MapIndex(k))
			if err != nil {
				return nil, nil, err
			}
			fields[k.String()] = value
		}
		return structpb.NewStructValue(&structpb.Struct{Fields: fields}), nil, nil
	default:
		return nil, nil, fmt.Errorf("%w %s", errUnsupported, v.Type())
	}
}

func schemaOf(t any) *typespb.FlagSchema {
	s := &typespb.FlagSchema{}
	switch t := t.(type) {
	case *typespb.FlagSchema_BoolSchema:
		s.SchemaType = t
	case *typespb.FlagSchema_StringSchema:
		s.SchemaType = t
	case *typespb.FlagSchema_DoubleSchema:
		s.SchemaType = t
	case *typespb.FlagSchema_StructSchema:
		s.SchemaType = t
	}
	return s
}

func intSchema() *typespb.FlagSchema {
	return &typespb.FlagSchema{SchemaType: &typespb.FlagSchema_IntSchema{IntSchema: &typespb.FlagSchema_IntFlagSchema{}}}
}

// decode converts an evaluated value into the type of def. For an untyped nil default it returns
// the value's natural Go representation.
func decode(value *structpb.Value, def any) (any, error) {
	if def == nil {
		return value.AsInterface(), nil
	}
	out, err := decodeTo(value, reflect.TypeOf(def), nil)
	if err != nil {
		return nil, err
	}
	return out.Interface(), nil
}

func decodeTo(value *structpb.Value, t reflect.Type, path []string) (reflect.Value, error) {
	if _, null := value.GetKind().(*structpb.Value_NullValue); null || value == nil {
		return reflect.Zero(t), nil
	}
	switch t.Kind() {
	case reflect.Pointer:
		elem, err := decodeTo(value, t.Elem(), path)
		if err != nil {
			return reflect.Value{}, err
		}
		p := reflect.New(t.Elem())
		p.Elem().Set(elem)
		return p, nil
	case reflect.Interface:
		return reflect.ValueOf(value.AsInterface()), nil
	case reflect.Bool:
		return reflect.ValueOf(value.GetBoolValue()).Convert(t), nil
	case reflect.String:
		return reflect.ValueOf(value.GetStringValue()).Convert(t), nil
	case reflect.Float32, reflect.Float64:
		return reflect.ValueOf(value.GetNumberValue()).Convert(t), nil
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return decodeInt(value, t, path)
	case reflect.Struct:
		fields := value.GetStructValue().GetFields()
		out := reflect.New(t).Elem()
		for i := range t.NumField() {
			f := t.Field(i)
			if !f.IsExported() {
				continue
			}
			name := fieldName(f)
			fv, err := decodeTo(fields[name], f.Type, append(path, name))
			if err != nil {
				return reflect.Value{}, err
			}
			out.Field(i).Set(fv)
		}
		return out, nil
	case reflect.Map:
		s, ok := value.GetKind().(*structpb.Value_StructValue)
		if !ok {
			return reflect.Value{}, mismatch(path, "map", value)
		}
		out := reflect.MakeMapWithSize(t, len(s.StructValue.GetFields()))
		for k, v := range s.StructValue.GetFields() {
			if err := checkKind(v, t.Elem(), append(path, k)); err != nil {
				return reflect.Value{}, err
			}
			ev, err := decodeTo(v, t.Elem(), append(path, k))
			if err != nil {
				return reflect.Value{}, err
			}
			out.SetMapIndex(reflect.ValueOf(k).Convert(t.Key()), ev)
		}
		return out, nil
	default:
		return reflect.Value{}, fmt.Errorf("%w %s", errUnsupported, t)
	}
}

// decodeInt checks what FlagSchema's single int kind can't: Go width and sign.
func decodeInt(value *structpb.Value, t reflect.Type, path []string) (reflect.Value, error) {
	n := value.GetNumberValue()
	if n != math.Trunc(n) {
		return reflect.Value{}, at(path, fmt.Errorf("%v is not a whole number", n))
	}
	out := reflect.New(t).Elem()
	if t.Kind() >= reflect.Uint && t.Kind() <= reflect.Uint64 {
		if n < 0 || out.OverflowUint(uint64(n)) {
			return reflect.Value{}, at(path, fmt.Errorf("%v overflows %s", n, t))
		}
		out.SetUint(uint64(n))
	} else {
		if out.OverflowInt(int64(n)) {
			return reflect.Value{}, at(path, fmt.Errorf("%v overflows %s", n, t))
		}
		out.SetInt(int64(n))
	}
	return out, nil
}

// checkKind validates a map element, which evaluate can't type-check (no schema for maps).
func checkKind(v *structpb.Value, t reflect.Type, path []string) error {
	for t.Kind() == reflect.Pointer {
		t = t.Elem()
	}
	ok := true
	switch v.GetKind().(type) {
	case *structpb.Value_NullValue:
	case *structpb.Value_BoolValue:
		ok = t.Kind() == reflect.Bool || t.Kind() == reflect.Interface
	case *structpb.Value_StringValue:
		ok = t.Kind() == reflect.String || t.Kind() == reflect.Interface
	case *structpb.Value_NumberValue:
		ok = (t.Kind() >= reflect.Int && t.Kind() <= reflect.Float64) || t.Kind() == reflect.Interface
	case *structpb.Value_StructValue:
		ok = t.Kind() == reflect.Struct || t.Kind() == reflect.Map || t.Kind() == reflect.Interface
	default:
		ok = t.Kind() == reflect.Interface
	}
	if !ok {
		return mismatch(path, t.String(), v)
	}
	return nil
}

func mismatch(path []string, want string, v *structpb.Value) error {
	return at(path, fmt.Errorf("resolved value (%T) not assignable to %s", v.GetKind(), want))
}

func at(path []string, err error) error {
	if len(path) == 0 {
		return err
	}
	return fmt.Errorf("at %s: %w", strings.Join(path, "."), err)
}

// fieldName is the flag-value field a Go struct field maps to: its json tag, else snake_case.
func fieldName(f reflect.StructField) string {
	if tag := f.Tag.Get("json"); tag != "" {
		if name, _, _ := strings.Cut(tag, ","); name != "" && name != "-" {
			return name
		}
	}
	return toSnakeCase(f.Name)
}

// toSnakeCase converts CamelCase to snake_case, keeping acronyms together: "HTTPServer" ->
// "http_server", "userID" -> "user_id".
func toSnakeCase(s string) string {
	runes := []rune(s)
	var b strings.Builder
	for i, r := range runes {
		if i > 0 && r >= 'A' && r <= 'Z' {
			prev := runes[i-1]
			lowerOrDigit := (prev >= 'a' && prev <= 'z') || (prev >= '0' && prev <= '9')
			acronymEnd := prev >= 'A' && prev <= 'Z' && i+1 < len(runes) && runes[i+1] >= 'a' && runes[i+1] <= 'z'
			if lowerOrDigit || acronymEnd {
				b.WriteByte('_')
			}
		}
		b.WriteRune(r)
	}
	return strings.ToLower(b.String())
}
